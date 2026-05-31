# FLTS device-sync modeling brief (Syncthing-based)

> Supersedes the pre-`feat/native-sync-v2` brief. FLTS no longer drives sync with
> an abstract "external sync system": it now embeds **Syncthing** as the transport
> and layers an **app-managed roster mesh** on top of a synced file. The old
> file-merge model survives but is re-grounded: its conflict siblings are now
> concretely Syncthing `.sync-conflict-*` files, and its `mtime` is now a
> cross-device wall clock rather than a single monotonic logical clock.

## 1. System overview

FLTS is a Rust workspace. Device sync has two layers:

1. **Transport (new):** `library/src/sync/` embeds Syncthing (Go c-archive, v1.30.0)
   over a tiny FFI (`syncthing-sys`) and drives it through its localhost REST API
   (`sync/control.rs`). One engine per process (`sync/engine.rs`), owned by the
   Tauri sync daemon (`site/src-tauri/src/app/sync_daemon.rs`). Syncthing replicates
   one folder — the app-private library root — between paired devices.
2. **Roster mesh (new):** `<library_root>/.flts/devices.json` is itself synced
   content. Every node lists *all* mesh devices; each node reconciles its Syncthing
   device set against the merged roster (`sync/roster.rs`, `sync/reconcile.rs`), so a
   pairing on **any** node propagates everywhere. The roster file is merged as a
   convergent CRDT and union-merges its own `.sync-conflict-*` siblings.
3. **File-merge layer (surviving, re-grounded):** the persistence layer detects and
   resolves Syncthing conflict siblings per object type — `book.dat` newest-mtime-wins,
   `state.json` newest-wins, `translation_*.dat` semantic merge, cards/dictionary union,
   `chapter_summaries.dat` union. (`library/src/library.rs`,
   `library/src/library/library_book/`, `library/src/book/translation.rs`,
   `library/src/library/library_card.rs`.)

**Category: A (Distributed / Message-Passing).** This is now literally a multi-device
replication mesh: the correctness boundaries are cross-device eventual consistency,
membership/reconfiguration convergence, and conflict reconciliation over an
asynchronous replicated filesystem — not thread-level atomics.

**Concurrency model:** async Rust (tokio). Three *independent control loops* per node
that the protocol depends on but never synchronizes: (a) Syncthing's own replication,
(b) the 10 s reconcile+status poller (`sync_daemon.rs:122-133`), (c) the library file
watcher (`library/src/library/file_watcher.rs`) feeding reload/save. Membership and
data both converge only across these loops.

**Key deviation from "just use Syncthing":** removal is *opt-in* — the roster carries
tombstones and a device merely absent from the roster is never torn down
(`reconcile.rs:42-50`). Timestamps for both the roster CRDT and `book.dat`/`state.json`
resolution are **per-device wall clocks** (`roster.rs:209` `now_ms`; filesystem mtime
that Syncthing preserves from the origin device), so causal ordering is approximated by
clock comparison.

## 2. Bug families

### Family 1: Roster CRDT last-writer-wins under per-device clocks

**Mechanism:** mesh membership converges by union-merging roster copies with
last-writer-wins per device, comparing an add timestamp (`addedAtMs`) against a removal
tombstone, where a tombstone wins **only if strictly newer** (`rts > rec.added_at_ms`).
All timestamps are independent wall clocks.

**Evidence:**
- Historical: `091ed40` (Phase 4: app-managed roster mesh — RosterStore + reconcile).
- Code analysis:
  - `library/src/sync/roster.rs:48-84` — `Roster::merge`: per-id newest-add vs newest-tombstone, tombstone wins iff strictly newer; equal timestamps → device stays active.
  - `roster.rs:142-163` — `add_device` clears the tombstone; `remove_device` drops the device and writes a tombstone; both stamp `now_ms()`.
  - `roster.rs:209-214` — `now_ms()` is local `SystemTime::now()`, no logical clock.
  - Unit tests `roster.rs:245-264` assert commutativity and re-add-beats-tombstone, but only with hand-picked, well-ordered timestamps.

**Affected code paths:** `Roster::merge`, `RosterStore::{add_device, remove_device, load}`.

**Suggested modeling approach:**
- Variables: per-node roster (`devices : id ↦ addedAt`, `removed : id ↦ removedAt`), a small set of device ids, an abstract clock per node.
- Actions: `PairOn(node, id)`, `UnpairOn(node, id)`, `RosterSync(src, dst)` (deliver one node's roster into another's merge), `ReaddOn`.
- Granularity: model the clock as a non-monotonic-across-nodes value so an add can carry a timestamp older/newer than a causally-earlier remove on another node.

**Priority:** High — core of the new protocol; convergence correctness rides on the LWW rule and on clock comparability.

### Family 2: Reconcile asymmetry — add-on-presence vs remove-only-on-tombstone

**Mechanism:** reconciliation ADDS every active roster device missing from the engine,
but REMOVES an engine device only if the roster *tombstones it and does not re-list it*.
A device absent from the roster is deliberately left in the engine. Self is never
added or removed.

**Evidence:**
- Historical: `091ed40`.
- Code analysis:
  - `library/src/sync/reconcile.rs:31-53` — `reconcile`: `to_add` = active roster − engine; `to_remove` = engine ∩ `roster.removed` ∖ `roster.devices`; both skip `my_id`.
  - `engine.rs:187-212` — `reconcile_once` applies the plan: add+reshare, then remove+reshare.
  - `engine.rs:178-181` — `set_device_name` → `ensure_self` re-adds self to the roster (new `addedAtMs`) whenever the name changed.

**Affected code paths:** `reconcile`, `SyncEngine::{reconcile_once, ensure_self via set_device_name}`.

**Suggested modeling approach:**
- Variables: per-node engine device set, plus the per-node merged roster from Family 1.
- Actions: `ReconcileNode(node)` deriving the plan and mutating the engine set.
- Granularity: keep `to_add`/`to_remove` as one atomic plan but allow nodes to reconcile in any interleaving relative to `RosterSync`.

**Priority:** High — this is the rule that turns roster state into actual folder sharing; its asymmetry is the intended safety guard (no tear-down from a node that hasn't learned an add) and the prime suspect for divergence when composed with Family 1.

### Family 3: Mesh propagation across independent, unsynchronized loops

**Mechanism:** a single pairing is supposed to fan out to a full mesh, but propagation
is the composition of three loops that share no clock: Syncthing replicates the roster
file, each node's 10 s poller runs `reconcile_once`, and only then does that node share
the folder with the newly-learned peer. The *first* link still requires a two-sided
pairing (manual approval of a pending device); reconcile only closes the mesh for the
3rd+ device.

**Evidence:**
- Historical: `091ed40` (mesh), `cfe863d` (pending-device approval), `83c4a16` (camera QR pairing).
- Code analysis:
  - `sync_daemon.rs:122-133` — poller: `reconcile_once` then `push_status`, every 10 s.
  - `engine.rs:111-157` — `add_peer`/`reshare_library`: share the folder with the full device set; a peer only receives the roster after it is in the folder's device list.
  - `sync.rs:127-165` — pairing is one-sided `pair_device`; the other half is the peer approving a *pending* device (`pending_devices`), which the daemon does **not** auto-approve.

**Affected code paths:** the poller loop, `reshare_library`, `pair_device`, `pending_devices`.

**Suggested modeling approach:**
- Variables: per-node engine device set + folder-share set; an in-flight roster "channel" per directed pair (models replication latency / loss).
- Actions: `DeliverRoster`, `ReconcileNode`, `ApprovePending`; allow arbitrary interleaving and message delay.
- Granularity: model replication as an asynchronous channel that can lag arbitrarily; this is where the *liveness* question lives.

**Priority:** High — this is the headline "pair once → full mesh" claim; it is an eventual-consistency / closure property, ideal for a liveness check under fairness.

### Family 4: Syncthing-fed conflict resolution (re-grounded merge layer)

**Mechanism:** Syncthing creates one whole-file `.sync-conflict-<date>-<time>-<modifiedBy>.<ext>`
sibling per concurrent modification of the same path and preserves each file's origin
mtime. FLTS resolves those siblings per object type with *different* rules — and "newest"
now compares mtimes stamped on different devices' clocks.

**Evidence:**
- Historical: `bb1b075`, `f2ac258`, `e104b4a`, `b793938` (original merge logic); re-grounded under Syncthing on this branch.
- Code analysis:
  - `library/src/library.rs:96-141` — book conflict discovery by name (`starts_with("book") && ends_with(".dat") && != "book.dat"`) — matches Syncthing's `book.sync-conflict-….dat`; filtered by embedded book id.
  - `library/src/library.rs:166-188` — translation siblings grouped by `chunk_by(id)` over **unsorted** `read_dir` order, shortest path = main.
  - `library_book/mod.rs:354-397` — `book.dat` newest-mtime-wins, move winner to `book.dat`, delete the rest.
  - `library_book/mod.rs:154-176` — translations: deserialize each sibling, `Translation::merge`, rewrite, delete siblings.
  - `library/src/book/translation.rs:399-479` — merge dedups versions by `timestamp`; same-second versions coalesce.
  - `library/src/card.rs:378` (`Card::merge`) / dictionary union — monotonic union contrast path.
  - `library/src/library/library_book/reading_state.rs:30-129` — `state.json` newest-wins.

**Affected code paths:** `BookMetadata` conflict discovery, `LibraryBook::load_from_metadata`, `LibraryTranslation::load_from_metadata`, `Translation::merge`, `Card::merge`, `resolve_reading_state_file`.

**Suggested modeling approach:**
- Variables: as the existing `spec/base.tla` (canonical + conflict siblings carrying content and mtime), but mtime drawn from a **per-device** clock domain.
- Actions: keep the existing `Inject*Conflict` / `Load*FromMetadata` actions; relabel injections as "Syncthing delivers a `.sync-conflict-*` sibling".
- Granularity: unchanged from base.tla; add a clock-skew dimension so "newest mtime" can disagree with causal order.

**Priority:** Medium for book/state (newest-wins is documented design — see § 5); High for translation version identity (carried `TranslationDistinctVersionsPreserved`).

### Family 5: Save → watch → reload echo loop and the content-hash gate

**Mechanism:** a local save bumps mtime, the watcher fires, reload runs, reload may
re-save; Syncthing re-touching a file bumps mtime without changing bytes. Without a
content check this is an unbounded `save → watch → reload → save` loop, and across two
devices the echoes ping-pong. The fix gates on a trailing FNV content hash so equal
content is recognized as the app's own (or Syncthing's) echo and dropped.

**Evidence:**
- Historical: `6e5e4f4` ("Fix infinite save/reload loop via content-hash echo gate").
- Code analysis:
  - `book/serialization.rs` — trailing 8-byte FNV hash; `read_stored_hash_from_path` for cheap on-disk fingerprint.
  - `library_book/mod.rs:441-459` — `reload_book`: quick mtime reject, then skip the re-save when on-disk hash == `last_saved_hash`.
  - `library_book/mod.rs:148-151,344-348` — `last_saved_hash` tracked on `LibraryBook`/`LibraryTranslation`, set on load and after each serialize.
  - `file_watcher.rs:173` — watcher already ignores `.sync-conflict-` and `~`-temp names.

**Affected code paths:** `reload_book`, `reload_translations`, `LibraryBook::save`, `LibraryTranslation::load`, the file watcher.

**Suggested modeling approach:**
- Variables: per-node in-memory content + content-hash, on-disk content + mtime + stored-hash, `last_saved_hash`.
- Actions: `LocalEdit`, `Save` (writes content+hash, bumps mtime), `WatcherTick`→`Reload` (re-save only if disk-hash ≠ last-saved-hash), `SyncthingReTouch` (bumps mtime, identical bytes), `DeliverContent(src,dst)`.
- Granularity: two-step save and a separate watcher tick; the property is **termination/quiescence**.

**Priority:** High — most recent real bug; a liveness/termination target distinct from the safety merge families.

## 3. Modeling recommendations

### 3.1 Model

| What | Why | How |
|---|---|---|
| Roster as a per-node CRDT with add/tombstone timestamps | Family 1 convergence rides on the LWW rule | `devices`/`removed` maps + `RosterSync` merge action |
| Per-device (non-monotonic-across-nodes) clock | Families 1 & 4 both depend on cross-device timestamp comparison | one clock var per node; allow skew |
| Engine device set + folder-share set per node | Families 2 & 3 act on these, not on the roster directly | derive via `ReconcileNode` from the merged roster |
| Asynchronous roster/content delivery channel | Family 3 liveness and Family 5 ping-pong need replication latency | per-directed-pair in-flight buffer that may lag |
| Content hash + mtime split on data files | Family 5 termination | distinct `content`, `mtime`, `storedHash`, `lastSavedHash` |
| Existing book/state/translation/dict conflict resolution | Family 4 re-grounding | keep `spec/base.tla` actions; relabel injection source as Syncthing |

### 3.2 Do not model

| What | Why |
|---|---|
| Syncthing's internal BEP/block protocol, indexes, deltas | Treat Syncthing as a trusted eventually-consistent transport that delivers whole files and creates conflict siblings; its internals aren't the FLTS bug source |
| REST/HTTP client mechanics (retry, JSON shapes, `control.rs`) | Implementation detail; the trait surface is the only thing the protocol relies on |
| FFI / engine lifecycle (`syncthing-sys`, start/stop, port picking) | Glue; one engine per process, not a protocol concern |
| QUIC-disable / discovery/relay options | Connectivity tuning (`cfe863d`, `5eba80a`), no effect on convergence logic |
| UI status mapping (`SyncState`, completion %) | Consumer of state; no safety/liveness content |

## 4. Proposed extensions

| Extension | Variables | Purpose | Bug family |
|---|---|---|---|
| Roster CRDT | `roster[node].devices`, `roster[node].removed`, `clock[node]` | Convergent membership under LWW + skew | Family 1 |
| Reconcile projection | `engineDevices[node]`, `shareSet[node]` | Engine set derived from merged roster (opt-in removal) | Family 2 |
| Replication channel | `inflight[src][dst]` (roster + content) | Async delivery / latency / mesh closure | Families 3, 5 |
| Pairing handshake | `pending[node]`, `paired[a][b]` | Two-sided first link before mesh fan-out | Family 3 |
| Content/hash file model | `disk[node].{content,mtime,hash}`, `mem[node].{content,hash}`, `lastSaved[node]` | Echo-gate termination | Family 5 |
| Syncthing conflict siblings | `*Main`, `*Conflicts` with per-device mtime (from base.tla) | Re-grounded merge resolution | Family 4 |

## 5. Proposed invariants

| Invariant | Type | Description | Targets |
|---|---|---|---|
| `RosterConvergence` | Safety | Nodes that have merged the same set of pair/unpair ops hold equal rosters (merge is commutative/idempotent) | Family 1 |
| `NoSpuriousResurrection` | Safety | A device whose latest causal op is a removal is never active in any node's merged roster | Family 1 |
| `ReconcileNeverDropsActive` | Safety | Reconcile never removes an engine device that is active in the merged roster (only tombstoned-and-absent) | Family 2 |
| `MeshClosure` | Liveness | Under fair delivery + reconcile, from one accepted pairing every node eventually shares the folder with every other | Families 2, 3 |
| `EchoQuiescence` | Liveness | After finitely many edits and fair delivery, the system reaches a fixpoint where no node re-saves | Family 5 |
| `TranslationDistinctVersionsPreserved` | Safety | Distinct translation version ids are not collapsed merely because timestamps collide | Family 4 |
| `DictionaryEntriesMonotonic` | Structural | Card/dictionary merge never loses a previously known entry | Family 4 |
| ~~`BookNewestWins` / `StateNewestWins`~~ | — | **Not invariants** — newest-wins for `book.dat`/`state.json` is documented design (no field-level merge possible) | Family 4 |

## 6. Findings pending verification

### 6.1 Model-checkable

| ID | Description | Expected invariant violation | Bug family |
|---|---|---|---|
| M1 | Under per-device clock skew, a remove on a behind-clock node carries an `addedAtMs`-comparable timestamp older than a concurrent (causally-earlier) add on an ahead-clock node, so the merge keeps the device active | `NoSpuriousResurrection` | Family 1 |
| M2 | A device paired only through an intermediary that tombstones (or never reshares) before the roster reaches a third node — does the third node still reach full sharing, or stay isolated? | `MeshClosure` | Families 2, 3 |
| M3 | Concurrent add+remove of the same device on two nodes with equal `now_ms()` (ms collision) resolve to "active" (`rts > added` is false at equality) — is that the intended, convergent outcome on every node? | `RosterConvergence` / `NoSpuriousResurrection` | Family 1 |
| M4 | Two devices make concurrent **divergent** edits and exchange them under the content-hash gate: each merge yields new bytes/hash, so the equal-hash echo break may not fire — does the system still quiesce or echo forever? | `EchoQuiescence` | Family 5 |
| M5 | Two translation siblings carry different version ids but identical second-level `timestamp`s; conflict merge dedups by timestamp | `TranslationDistinctVersionsPreserved` | Family 4 |

### 6.2 Test-verifiable

| ID | Description | Suggested test approach |
|---|---|---|
| T1 | Translation conflict grouping uses `chunk_by(id)` over **unsorted** `read_dir` order; non-adjacent same-id siblings form separate groups and skip the merge | Integration test: place `translation_A.dat`, `translation_B.dat`, `translation_A.sync-conflict-….dat` so the A copies are non-adjacent; assert the conflict is merged, not orphaned (`library/src/library.rs:166-188`) |
| T2 | Roster re-add immediately after remove with equal-ms timestamps | Drop the `sleep(2ms)` in `roster.rs` `add_then_remove_then_readd` and assert the re-add still wins (or document that it must not be equal-ms) |
| T3 | `book.dat` newest-mtime-wins picks the causally-older edit when the winner's origin clock is behind | Two-dir fixture with mtimes inverted vs. write order; confirm/triage behavior |

### 6.3 Code-review-only

| ID | Description | Suggested action |
|---|---|---|
| C1 | Both the roster CRDT and `book.dat`/`state.json` resolution order by **wall-clock** timestamps (`now_ms`, preserved mtime). Decide if clock skew is acceptable or a logical clock / version vector is warranted | Design review |
| C2 | `chunk_by(id)` in translation discovery assumes id-adjacency; `read_dir` order is unspecified | Sort by id before `chunk_by`, or group with a map (`library/src/library.rs:166-171`) |
| C3 | The daemon poller never auto-approves pending devices; the first pairing needs manual two-sided approval. Confirm this is intended (QR flow) and that no mesh case relies on auto-approval | Confirm product intent |
| C4 | `ensure_self` re-adds self with a fresh `addedAtMs` on a name change — confirm it cannot resurrect a self that another node legitimately tombstoned | Review `set_device_name` vs tombstones |

## 7. Reference pointers

- Full analysis report: `spec/analysis-report.md`
- New transport + mesh:
  - `library/src/sync/roster.rs:48-196`
  - `library/src/sync/reconcile.rs:31-53`
  - `library/src/sync/engine.rs:70-242`
  - `library/src/sync/control.rs:107-150`
  - `site/src-tauri/src/app/sync_daemon.rs:118-218`
  - `site/src-tauri/src/app/sync.rs:127-181`
  - `syncthing-sys/src/lib.rs`, `syncthing-core/wrapper.go`
- Re-grounded merge layer:
  - `library/src/library.rs:96-233`
  - `library/src/library/library_book/mod.rs:154-176, 354-459`
  - `library/src/book/translation.rs:399-479`
  - `library/src/library/library_book/reading_state.rs:30-129`
  - `library/src/book/serialization.rs` (trailing FNV hash)
- Existing merge TLA+ spec (re-grounded, not replaced): `spec/base.tla`, `spec/MC.tla`, `spec/Trace.tla`
- Relevant history: `f8066b7`, `e85a6a1`, `e35fccb`, `5eba80a`, `6b11e2e`, `091ed40`, `c36544f`, `83c4a16`, `cfe863d`, `ff3300b`, `364c1bc`, `6e5e4f4`
- Reference: [Syncthing conflict handling](https://docs.syncthing.net/users/syncing.html#conflicting-changes) — `.sync-conflict-<date>-<time>-<modifiedBy>.<ext>`, mtime preserved per file. Embedding is MPL-2.0, App-Store-safe (see memory `reference_syncthing_licensing`).
- No GitHub issues/PRs: `TSultanov/flts` has no issue tracker activity for this work; the 15 branch commits are the entire history.
