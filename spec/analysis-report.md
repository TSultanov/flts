# FLTS device-sync code analysis report (Syncthing migration)

Audit trail for the re-analysis that produced `spec/modeling-brief.md` after FLTS
replaced its abstract sync layer with embedded Syncthing on `feat/native-sync-v2`.
Scope requested: **full re-analysis**, covering **both** the new roster mesh and the
re-grounded file-merge layer.

## Step 0 — Category

**Category A (Distributed / Message-Passing).** Post-migration FLTS is a multi-device
replication mesh: correctness lives in cross-device eventual consistency, membership
reconfiguration, and conflict reconciliation over an async replicated filesystem. Not
BFT (no Byzantine threat model — paired devices are mutually trusted); `bft-analysis.md`
does not apply. Crash/recovery is secondary (a node restart re-reads the synced folder
and the persisted Syncthing config); the dominant adversaries are message delay/reorder
(replication latency) and clock skew (wall-clock timestamps).

## Phase 1 — Reconnaissance

Architecture (three layers):

| Layer | Files | Role |
|---|---|---|
| FFI | `syncthing-sys/src/lib.rs`, `syncthing-core/wrapper.go` | `start`/`stop`/`ping` into the Go c-archive (Syncthing v1.30.0), static-linked |
| Transport control | `library/src/sync/control.rs` | `SyncthingApi` trait (HTTP + mock); devices/folders/options/pending/completion over REST |
| Engine + mesh | `library/src/sync/engine.rs`, `roster.rs`, `reconcile.rs` | one engine/process; roster CRDT + reconcile-to-mesh |
| Daemon | `site/src-tauri/src/app/sync_daemon.rs`, `sync.rs` | owns the engine; 10 s reconcile+status poller; Tauri pairing commands |
| Merge (re-grounded) | `library/src/library.rs`, `library/src/library/library_book/`, `book/translation.rs`, `library_card.rs`, `book/serialization.rs` | resolves Syncthing `.sync-conflict-*` siblings |

Independent control loops (no shared clock): Syncthing replication · the 10 s
reconcile/status poller (`sync_daemon.rs:122-133`) · the library file watcher
(`file_watcher.rs`). Membership and data both converge only across these loops.

## Phase 2 — Bug archaeology

- **Git:** 15 commits, `main..HEAD`. The phased build-out (`f8066b7` FFI → `e85a6a1`
  private library → `e35fccb` control+engine+daemon → `5eba80a` dynamic BEP ports →
  `6b11e2e` pairing UI → `091ed40` roster mesh → `c36544f` Docker multi-node tests →
  `83c4a16` iOS QR pairing → `cfe863d` QUIC-disable + pending approval → `ff3300b`
  device naming → `364c1bc` foreground reconnect → `6e5e4f4` echo-gate loop fix).
- **Bug-fix commits (mechanism-bearing):**
  - `6e5e4f4` — infinite `save→watch→reload→save` loop; mtime-only reload was
    content-blind and atomic saves / Syncthing re-touches bump mtime without changing
    bytes. → Family 5.
  - `cfe863d` — QUIC handshake panic took down the in-process engine (quic-go pinned by
    Syncthing); TCP-only is now the production default. Connectivity, not protocol logic
    → excluded from modeling (brief § 3.2).
  - `ff3300b` — device naming propagated through both the roster and Syncthing's
    announced name. → touches Family 1 (`ensure_self` resurrection edge, C4).
- **GitHub issues/PRs:** none. `TSultanov/flts` has no relevant issue-tracker activity;
  the branch commits are the entire record. (Consistent with the prior brief's "no
  matching issues" note.)
- **Hotspot:** `roster.rs` + `reconcile.rs` + `engine.rs` (the new protocol) and the
  `library_book`/`library.rs` conflict-discovery code are the dense areas.

## Phase 3 — Deep analysis (verified findings)

1. **Roster LWW (`roster.rs:48-84`).** Per-id newest-add vs newest-tombstone; tombstone
   wins iff `rts > rec.added_at_ms`. Commutative/idempotent **given the timestamps**, but
   timestamps are independent wall clocks (`now_ms`, `roster.rs:209`). Convergence to a
   single value is guaranteed; convergence to the *causally correct* value is not under
   skew. → Family 1; M1, M3, C1.
2. **Reconcile asymmetry (`reconcile.rs:31-53`).** Adds active-roster∖engine; removes only
   `engine ∩ removed ∖ devices`; never self. Absent≠remove is a deliberate guard against
   tear-down by a node that hasn't learned an add. → Family 2; M2.
3. **Mesh propagation (`engine.rs:111-212`, `sync_daemon.rs:122-133`).** `pair_device`
   writes the roster + adds locally; peers converge only after the roster file replicates
   and their poller runs `reconcile_once` + reshares. First link still needs a two-sided
   handshake (manual pending approval, `sync.rs:147-165`); reconcile closes the mesh for
   the 3rd+ node. Eventual-consistency/closure property. → Family 3; M2, C3.
4. **Re-grounded merge (`library.rs:96-188`, `library_book/mod.rs:154-176,354-397`).**
   Conflict discovery by Syncthing's naming convention; book newest-mtime-wins; translation
   semantic merge dedup-by-timestamp (`translation.rs:399-479`); card/dictionary union.
   "Newest mtime" now compares cross-device mtimes Syncthing preserves. → Family 4; M4(=M5
   in brief numbering), T3, C1.
   - **Latent discovery bug:** translation siblings are grouped with `chunk_by(id)` over
     **unsorted** `read_dir` order (`library.rs:166-171`); `chunk_by` only groups
     *consecutive* equal keys, so non-adjacent same-id siblings split into separate groups
     and skip the merge. → T1, C2.
5. **Echo gate (`6e5e4f4`, `library_book/mod.rs:441-459`, `serialization.rs`).** Trailing
   8-byte FNV content hash; reload skips re-save when on-disk hash == `last_saved_hash`.
   Breaks the equal-content echo. **Open:** under concurrent *divergent* two-device edits,
   each merge produces new bytes/hash, so the equal-hash break may not fire — does the
   system still quiesce? → Family 5; M4.

Excluded as non-modeling (verified): REST/JSON mechanics and retry (`control.rs`), FFI and
engine lifecycle, QUIC/discovery options, UI status mapping. Rationale in brief § 3.2.

## Phase 4 — Outputs

- `spec/modeling-brief.md` — rewritten; 5 bug families, both subsystems, Category A,
  extensions/invariants/findings.
- `spec/base.tla` — header + injection-action comments re-grounded to name Syncthing as
  the concrete conflict source and flag the cross-device-clock dimension; **action
  semantics unchanged** (still the verified Family-4 merge model).
- **Not done here (next phase = spec_generation + validation-workflow):** authoring and
  TLC-checking the new roster-mesh TLA+ module(s) for Families 1-3 & 5. The brief §§ 3-6
  give the concrete variables, actions, and invariants (`RosterConvergence`,
  `NoSpuriousResurrection`, `ReconcileNeverDropsActive`, `MeshClosure`, `EchoQuiescence`)
  to generate them directly.
