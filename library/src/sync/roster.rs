//! App-managed device roster — the mesh membership list.
//!
//! Lives at `<library_root>/.flts/devices.json` and syncs as ordinary library
//! content. Every node lists *all* mesh devices (including itself); a peer that
//! receives the roster reconciles it against its own engine, so adding a device
//! on any node propagates everywhere (see [`super::reconcile`]).
//!
//! Because the file syncs, concurrent edits on two nodes can produce Syncthing
//! `.sync-conflict-*` siblings. [`RosterStore::load`] union-merges them and
//! cleans them up — the same approach the card store uses
//! ([`crate::library::library_card`]).
//!
//! ## Causal merge (remove-wins)
//!
//! Membership is a **vector-clock CRDT**, not wall-clock last-writer-wins. Each
//! device id carries an *add context* and a *remove context* (both [`VClock`]s).
//! A local add/remove on this node bumps this node's component of the roster's
//! causal context and stamps the relevant context. Merge joins the two contexts
//! pointwise (a semilattice → commutative, associative, idempotent), and a device
//! is **present iff its add context strictly dominates its remove context**. This
//! is *remove-wins*: a removal that causally follows the add it observed always
//! wins (regardless of wall-clock skew — the bug this replaced, see
//! `spec/roster/bug-report.md`), and a concurrent removal also wins because it
//! carries its origin node's component that the adds cannot cover. A later re-add
//! observes the tombstone, so its context dominates and the device comes back.
//!
//! The legacy `devices`/`removed` fields are kept (and serialized) so a
//! not-yet-upgraded node can still read the file; new nodes treat them as a
//! derived view and seed empty contexts from them (see [`Roster::normalize`]).

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use log::warn;
use serde::{Deserialize, Serialize};

/// A version vector: `deviceId -> logical counter`. Missing keys are 0.
pub type VClock = BTreeMap<String, u64>;

/// `a` dominates `b`: `a[k] >= b[k]` for every component (missing = 0).
fn vc_dominates(a: &VClock, b: &VClock) -> bool {
    b.iter().all(|(k, vb)| a.get(k).copied().unwrap_or(0) >= *vb)
}

/// `a` strictly dominates `b`: dominates and is not equal.
fn vc_strictly_dominates(a: &VClock, b: &VClock) -> bool {
    a != b && vc_dominates(a, b)
}

/// Pointwise maximum (the lattice join). Result is canonical (no zero entries).
fn vc_join(a: &VClock, b: &VClock) -> VClock {
    let mut out = a.clone();
    for (k, vb) in b {
        let e = out.entry(k.clone()).or_insert(0);
        *e = (*e).max(*vb);
    }
    vc_canon(&mut out);
    out
}

/// Drop zero components so a clock has a single canonical form (`{A:0}` ≡ `{}`).
/// Without this two encodings of the same clock would mutually "strictly
/// dominate" and break commutativity of [`Roster::merge`].
fn vc_canon(vc: &mut VClock) {
    vc.retain(|_, v| *v > 0);
}

/// One device in the legacy compatibility view.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeviceRecord {
    pub name: String,
    #[serde(rename = "addedAtMs")]
    pub added_at_ms: u64,
}

/// Authoritative add stamp: causal context of the add plus advisory metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct AddStamp {
    pub name: String,
    #[serde(rename = "addedAtMs", default)]
    pub added_at_ms: u64,
    #[serde(default)]
    pub vc: VClock,
}

/// Authoritative remove stamp: causal context of the removal plus advisory ms.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RemStamp {
    #[serde(rename = "removedAtMs", default)]
    pub removed_at_ms: u64,
    #[serde(default)]
    pub vc: VClock,
}

/// The full mesh membership. `adds`/`removes` are authoritative (causal); the
/// `devices`/`removed` maps are a derived view kept in sync for old-schema
/// readers (see module docs). All fields `#[serde(default)]` so old files parse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Roster {
    /// Derived view: currently-present devices (for not-yet-upgraded readers).
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceRecord>,
    /// Derived view: tombstoned devices `deviceId -> removed_at_ms` (legacy).
    #[serde(default)]
    pub removed: BTreeMap<String, u64>,
    /// Authoritative add context per device.
    #[serde(default)]
    pub adds: BTreeMap<String, AddStamp>,
    /// Authoritative remove context per device.
    #[serde(default)]
    pub removes: BTreeMap<String, RemStamp>,
}

impl Roster {
    /// Whether a device is currently a member: its add context must strictly
    /// dominate its remove context (remove-wins on concurrency). When both
    /// contexts are empty (a pure-legacy record from an old node) fall back to
    /// the old wall-clock rule so old data keeps its prior meaning until a new
    /// node re-stamps it.
    pub fn is_present(&self, id: &str) -> bool {
        match (self.adds.get(id), self.removes.get(id)) {
            (Some(a), Some(r)) => {
                if a.vc.is_empty() && r.vc.is_empty() {
                    a.added_at_ms >= r.removed_at_ms // legacy LWW
                } else {
                    vc_strictly_dominates(&a.vc, &r.vc)
                }
            }
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// The roster's causal context = join of every add and remove context. The
    /// next local operation bumps this node's component of it.
    pub fn context(&self) -> VClock {
        let mut ctx = VClock::new();
        for a in self.adds.values() {
            ctx = vc_join(&ctx, &a.vc);
        }
        for r in self.removes.values() {
            ctx = vc_join(&ctx, &r.vc);
        }
        ctx
    }

    /// Seed authoritative `adds`/`removes` from the legacy `devices`/`removed`
    /// view when reading a file an old node wrote (empty contexts), then rebuild
    /// the derived view so the two are consistent. Idempotent.
    pub fn normalize(&mut self) {
        for (id, rec) in &self.devices {
            self.adds.entry(id.clone()).or_insert_with(|| AddStamp {
                name: rec.name.clone(),
                added_at_ms: rec.added_at_ms,
                vc: VClock::new(),
            });
        }
        for (id, ms) in &self.removed {
            self.removes.entry(id.clone()).or_insert_with(|| RemStamp {
                removed_at_ms: *ms,
                vc: VClock::new(),
            });
        }
        // Canonicalize every clock (drop zero components) so comparisons are
        // well-defined regardless of how the file encoded them.
        for a in self.adds.values_mut() {
            vc_canon(&mut a.vc);
        }
        for r in self.removes.values_mut() {
            vc_canon(&mut r.vc);
        }
        self.rebuild_view();
    }

    /// Recompute the legacy `devices`/`removed` view from the authoritative
    /// `adds`/`removes` + the presence rule.
    fn rebuild_view(&mut self) {
        let ids: BTreeSet<String> = self
            .adds
            .keys()
            .chain(self.removes.keys())
            .cloned()
            .collect();
        let mut devices = BTreeMap::new();
        let mut removed = BTreeMap::new();
        for id in ids {
            if self.is_present(&id) {
                if let Some(a) = self.adds.get(&id) {
                    devices.insert(
                        id,
                        DeviceRecord {
                            name: a.name.clone(),
                            added_at_ms: a.added_at_ms,
                        },
                    );
                }
            } else if let Some(r) = self.removes.get(&id) {
                removed.insert(id, r.removed_at_ms);
            }
        }
        self.devices = devices;
        self.removed = removed;
    }

    /// Union-merge two roster copies. Per device, the add and remove contexts are
    /// joined pointwise; presence is then recomputed. Commutative, associative,
    /// and idempotent (pointwise max is a semilattice).
    pub fn merge(&self, other: &Roster) -> Roster {
        let mut a = self.clone();
        a.normalize();
        let mut b = other.clone();
        b.normalize();

        let mut merged = Roster::default();
        let add_ids: BTreeSet<&String> = a.adds.keys().chain(b.adds.keys()).collect();
        for id in add_ids {
            merged
                .adds
                .insert(id.clone(), join_add(a.adds.get(id), b.adds.get(id)));
        }
        let rem_ids: BTreeSet<&String> = a.removes.keys().chain(b.removes.keys()).collect();
        for id in rem_ids {
            merged
                .removes
                .insert(id.clone(), join_rem(a.removes.get(id), b.removes.get(id)));
        }
        merged.rebuild_view();
        merged
    }
}

/// Join two add stamps (one may be absent): the causal context is the pointwise
/// max (authoritative, decides presence). The advisory `name`/`added_at_ms` are
/// resolved by plain max — a commutative/associative monoid, so the whole merge
/// converges byte-for-byte regardless of order. (These fields are display-only;
/// a join can synthesize a clock no single op holds, so a domination-based pick
/// would not be associative — see the convergence proptest.)
fn join_add(a: Option<&AddStamp>, b: Option<&AddStamp>) -> AddStamp {
    match (a, b) {
        (Some(a), Some(b)) => AddStamp {
            name: a.name.clone().max(b.name.clone()),
            added_at_ms: a.added_at_ms.max(b.added_at_ms),
            vc: vc_join(&a.vc, &b.vc),
        },
        (Some(a), None) => a.clone(),
        (None, Some(b)) => b.clone(),
        (None, None) => AddStamp::default(),
    }
}

/// Join two remove stamps: contexts max'd, advisory ms = max.
fn join_rem(a: Option<&RemStamp>, b: Option<&RemStamp>) -> RemStamp {
    match (a, b) {
        (Some(a), Some(b)) => RemStamp {
            removed_at_ms: a.removed_at_ms.max(b.removed_at_ms),
            vc: vc_join(&a.vc, &b.vc),
        },
        (Some(a), None) => a.clone(),
        (None, Some(b)) => b.clone(),
        (None, None) => RemStamp::default(),
    }
}

/// Reads, merges, and persists the roster file.
pub struct RosterStore {
    dir: PathBuf,
    path: PathBuf,
    /// This node's device id — the vector-clock component a local op increments.
    node_id: String,
}

const ROSTER_BASENAME: &str = "devices";
const ROSTER_FILENAME: &str = "devices.json";

impl RosterStore {
    /// Roster under `<library_root>/.flts/devices.json`, owned by `node_id` (this
    /// device's Syncthing id — the vector-clock component local ops advance).
    pub fn new(library_root: &Path, node_id: &str) -> Self {
        let dir = library_root.join(".flts");
        let path = dir.join(ROSTER_FILENAME);
        Self {
            dir,
            path,
            node_id: node_id.to_string(),
        }
    }

    /// Loads the roster, union-merging any `.sync-conflict-*` siblings and then
    /// writing the merged result back and deleting the siblings.
    pub fn load(&self) -> Result<Roster> {
        let mut roster = read_roster(&self.path).unwrap_or_default();
        roster.normalize();

        let siblings = self.conflict_siblings();
        if siblings.is_empty() {
            return Ok(roster);
        }
        for sib in &siblings {
            if let Some(other) = read_roster(sib) {
                roster = roster.merge(&other);
            }
        }
        // Persist the merged roster, then clear the siblings (best effort).
        self.save(&roster)?;
        for sib in &siblings {
            if let Err(err) = fs::remove_file(sib) {
                warn!("roster: could not remove conflict sibling {sib:?}: {err}");
            }
        }
        Ok(roster)
    }

    /// Atomically writes the roster (temp file + rename; the temp name carries a
    /// `~` so the library watcher ignores it).
    pub fn save(&self, roster: &Roster) -> Result<()> {
        fs::create_dir_all(&self.dir)
            .with_context(|| format!("creating roster dir {:?}", self.dir))?;
        let tmp = self.dir.join(format!("{ROSTER_FILENAME}~{}", now_ms()));
        let json = serde_json::to_vec_pretty(roster)?;
        fs::write(&tmp, json).with_context(|| format!("writing roster temp {tmp:?}"))?;
        fs::rename(&tmp, &self.path).with_context(|| format!("renaming roster into {:?}", self.path))?;
        Ok(())
    }

    /// This node's causal context for `roster`, advanced by one local tick — the
    /// vector clock to stamp on the operation about to be issued. Bumping this
    /// node's own component over the join of everything seen guarantees the new
    /// op strictly dominates whatever it observed for this device.
    fn next_vc(&self, roster: &Roster) -> VClock {
        let mut vc = roster.context();
        *vc.entry(self.node_id.clone()).or_insert(0) += 1;
        vc
    }

    /// Adds or refreshes a device. The new add carries a context that dominates
    /// any tombstone it observed, so it wins (re-add works). Returns the saved
    /// roster.
    pub fn add_device(&self, device_id: &str, name: &str) -> Result<Roster> {
        let mut roster = self.load()?;
        let vc = self.next_vc(&roster);
        roster.adds.insert(
            device_id.to_string(),
            AddStamp {
                name: name.to_string(),
                added_at_ms: now_ms(),
                vc,
            },
        );
        roster.rebuild_view();
        self.save(&roster)?;
        Ok(roster)
    }

    /// Tombstones a device (opt-in removal). The removal carries a context that
    /// dominates the add it observed → remove-wins. Returns the saved roster.
    pub fn remove_device(&self, device_id: &str) -> Result<Roster> {
        let mut roster = self.load()?;
        let vc = self.next_vc(&roster);
        roster.removes.insert(
            device_id.to_string(),
            RemStamp {
                removed_at_ms: now_ms(),
                vc,
            },
        );
        roster.rebuild_view();
        self.save(&roster)?;
        Ok(roster)
    }

    /// Ensures this device is listed (so peers learn about it). No-op if already
    /// active; updates the name if it changed.
    pub fn ensure_self(&self, my_id: &str, name: &str) -> Result<Roster> {
        let roster = self.load()?;
        let needs_write = match roster.devices.get(my_id) {
            Some(rec) => rec.name != name,
            None => true,
        };
        if needs_write {
            return self.add_device(my_id, name);
        }
        Ok(roster)
    }

    /// The `modifiedBy` device id of each pending `.sync-conflict-*` sibling — the
    /// merge sources the next [`load`](Self::load) will fold in. Trace-only: lets
    /// the engine emit one `RosterSync` event per incoming roster before the load
    /// clears the siblings. The id is the last `-` segment of the Syncthing name
    /// `devices.sync-conflict-<date>-<time>-<modifiedBy>.json`.
    /// The canonical roster on disk WITHOUT merging siblings — the pre-`load`
    /// state, so the engine can tell whether a `load` actually changed anything
    /// (and thus whether to emit `RosterSync`). Trace-only.
    #[cfg(feature = "tla_trace")]
    pub(crate) fn snapshot_for_trace(&self) -> Roster {
        read_roster(&self.path).unwrap_or_default()
    }

    #[cfg(feature = "tla_trace")]
    pub(crate) fn pending_sibling_sources(&self) -> Vec<String> {
        self.conflict_siblings()
            .iter()
            .filter_map(|p| {
                let stem = p.file_stem()?.to_str()?; // strips ".json"
                stem.rsplit('-').next().map(str::to_string)
            })
            .collect()
    }

    /// `.flts/devices.sync-conflict-*.json` siblings, sorted for determinism.
    fn conflict_siblings(&self) -> Vec<PathBuf> {
        let prefix = format!("{ROSTER_BASENAME}.sync-conflict-");
        let mut out = Vec::new();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return out;
        };
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(&prefix) && name.ends_with(".json") {
                out.push(entry.path());
            }
        }
        out.sort();
        out
    }
}

fn read_roster(path: &Path) -> Option<Roster> {
    let bytes = fs::read(path).ok()?;
    match serde_json::from_slice::<Roster>(&bytes) {
        Ok(roster) => Some(roster),
        Err(err) => {
            warn!("roster: ignoring unparseable {path:?}: {err}");
            None
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn vc(pairs: &[(&str, u64)]) -> VClock {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }
    fn add(name: &str, ts: u64, v: VClock) -> AddStamp {
        AddStamp {
            name: name.into(),
            added_at_ms: ts,
            vc: v,
        }
    }
    fn rem(ts: u64, v: VClock) -> RemStamp {
        RemStamp {
            removed_at_ms: ts,
            vc: v,
        }
    }
    /// A roster holding a single device's add and/or remove stamp.
    fn one(id: &str, a: Option<AddStamp>, r: Option<RemStamp>) -> Roster {
        let mut roster = Roster::default();
        if let Some(a) = a {
            roster.adds.insert(id.into(), a);
        }
        if let Some(r) = r {
            roster.removes.insert(id.into(), r);
        }
        roster.rebuild_view();
        roster
    }

    #[test]
    fn merge_unions_present_devices() {
        let a = one("A", Some(add("a", 1, vc(&[("A", 1)]))), None)
            .merge(&one("B", Some(add("b", 1, vc(&[("B", 1)]))), None));
        let merged = a.merge(&one("C", Some(add("c", 1, vc(&[("C", 1)]))), None));
        assert_eq!(merged.devices.keys().collect::<Vec<_>>(), vec!["A", "B", "C"]);
        assert!(merged.removed.is_empty());
    }

    #[test]
    fn dominant_remove_wins_despite_clock_skew() {
        // node A added X at vc{A:1}, wall-clock ms 100.
        let added = one("X", Some(add("x", 100, vc(&[("A", 1)]))), None);
        // node B observed that add, then removed X — vc{A:1,B:1} dominates the add
        // — but B's wall clock is BEHIND, so removed_at_ms (0) < added_at_ms (100).
        let removed = one("X", None, Some(rem(0, vc(&[("A", 1), ("B", 1)]))));

        let ab = added.merge(&removed);
        let ba = removed.merge(&added);
        assert_eq!(ab, ba, "merge is commutative");
        // The old wall-clock rule would resurrect X here; causal order does not.
        assert!(!ab.is_present("X"), "causally-later removal wins under skew");
        assert!(ab.removed.contains_key("X"));
    }

    #[test]
    fn concurrent_add_and_remove_remove_wins() {
        // A adds X (didn't see B); C removes X (didn't see the add) — concurrent.
        let added = one("X", Some(add("x", 5, vc(&[("A", 1)]))), None);
        let removed = one("X", None, Some(rem(5, vc(&[("C", 1)]))));
        let m = added.merge(&removed);
        assert!(!m.is_present("X"), "remove-wins on concurrency");
    }

    #[test]
    fn re_add_after_remove_resurrects() {
        // X removed at vc{A:1,B:1}; a re-add that observed it (vc{A:1,B:2}) wins.
        let removed = one("X", Some(add("x", 1, vc(&[("A", 1)]))), Some(rem(2, vc(&[("A", 1), ("B", 1)]))));
        assert!(!removed.is_present("X"));
        let readd = one("X", Some(add("x", 3, vc(&[("A", 1), ("B", 2)]))), None);
        let m = removed.merge(&readd);
        assert!(m.is_present("X"), "causally-later re-add resurrects");
    }

    #[test]
    fn load_merges_and_clears_conflict_siblings() {
        let tmp = std::env::temp_dir().join(format!("flts-roster-{}", now_ms()));
        let store = RosterStore::new(&tmp, "SELF");
        store.add_device("A", "alpha").unwrap();

        // A Syncthing conflict sibling written by another node adds B.
        let other = one("B", Some(add("beta", now_ms(), vc(&[("OTHER", 1)]))), None);
        let sibling = tmp
            .join(".flts")
            .join("devices.sync-conflict-20260530-120000-OTHER.json");
        fs::create_dir_all(sibling.parent().unwrap()).unwrap();
        fs::write(&sibling, serde_json::to_vec(&other).unwrap()).unwrap();

        let merged = store.load().unwrap();
        assert!(merged.devices.contains_key("A"));
        assert!(merged.devices.contains_key("B"), "sibling merged in");
        assert!(!sibling.exists(), "sibling cleaned up");
        assert!(store.load().unwrap().devices.contains_key("B"), "merge persisted");

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn add_remove_readd_via_store_no_sleep() {
        // The causal clock removes the need for the old sleep(2ms) hack.
        let tmp = std::env::temp_dir().join(format!("flts-roster-rr-{}", now_ms()));
        let store = RosterStore::new(&tmp, "SELF");

        store.add_device("P", "peer").unwrap();
        assert!(store.load().unwrap().devices.contains_key("P"));
        store.remove_device("P").unwrap();
        let r = store.load().unwrap();
        assert!(!r.devices.contains_key("P"));
        assert!(r.removed.contains_key("P"));
        store.add_device("P", "peer").unwrap();
        let r = store.load().unwrap();
        assert!(r.devices.contains_key("P"), "re-add resurrects");
        assert!(!r.removed.contains_key("P"));

        let _ = fs::remove_dir_all(&tmp);
    }

    // ---- Upgrade / mixed-version compatibility ----

    #[test]
    fn legacy_json_deserializes_and_normalizes() {
        // A file written by an old node: only devices/removed, no vc fields.
        let legacy = r#"{"devices":{"A":{"name":"alpha","addedAtMs":10}},"removed":{"B":5}}"#;
        let mut roster: Roster = serde_json::from_str(legacy).unwrap();
        roster.normalize();
        assert!(roster.is_present("A"), "legacy active device stays present");
        assert!(!roster.is_present("B"), "legacy tombstone stays removed");
        assert!(roster.adds["A"].vc.is_empty(), "seeded with empty (legacy) vc");
    }

    #[test]
    fn legacy_add_loses_to_new_remove() {
        // Legacy add (empty vc) vs a new remove that carries a real vc → removed.
        let legacy_add = one("X", Some(add("x", 100, VClock::new())), None);
        let new_remove = one("X", None, Some(rem(0, vc(&[("B", 1)]))));
        assert!(!legacy_add.merge(&new_remove).is_present("X"));
    }

    #[test]
    fn new_node_op_restamps_legacy_entry() {
        // A new node loads a legacy file, then issues an op → the entry gains a vc
        // (self-healing), and the causal rules apply from then on.
        let tmp = std::env::temp_dir().join(format!("flts-roster-heal-{}", now_ms()));
        fs::create_dir_all(tmp.join(".flts")).unwrap();
        fs::write(
            tmp.join(".flts").join("devices.json"),
            r#"{"devices":{"P":{"name":"peer","addedAtMs":1}},"removed":{}}"#,
        )
        .unwrap();
        let store = RosterStore::new(&tmp, "SELF");
        store.remove_device("P").unwrap();
        let r = store.load().unwrap();
        assert!(!r.is_present("P"));
        assert!(!r.removes["P"].vc.is_empty(), "removal carries a real vc now");
        let _ = fs::remove_dir_all(&tmp);
    }

    // ---- Convergence (the property the wall-clock merge lacked) ----

    fn arb_vc() -> impl Strategy<Value = VClock> {
        prop::collection::btree_map("[A-C]", 0u64..3, 0..3)
    }
    fn arb_roster() -> impl Strategy<Value = Roster> {
        let adds = prop::collection::btree_map(
            "[X-Z]",
            (0u64..3, arb_vc()).prop_map(|(ts, v)| add("n", ts, v)),
            0..3,
        );
        let removes = prop::collection::btree_map("[X-Z]", (0u64..3, arb_vc()).prop_map(|(ts, v)| rem(ts, v)), 0..3);
        (adds, removes).prop_map(|(adds, removes)| {
            let mut r = Roster {
                adds,
                removes,
                ..Default::default()
            };
            r.rebuild_view();
            r
        })
    }

    proptest! {
        #[test]
        fn merge_is_commutative(a in arb_roster(), b in arb_roster()) {
            prop_assert_eq!(a.merge(&b), b.merge(&a));
        }

        #[test]
        fn merge_is_associative(a in arb_roster(), b in arb_roster(), c in arb_roster()) {
            prop_assert_eq!(a.merge(&b).merge(&c), a.merge(&b.merge(&c)));
        }

        #[test]
        fn merge_is_idempotent(a in arb_roster()) {
            prop_assert_eq!(a.merge(&a), a.merge(&Roster::default()));
        }
    }
}
