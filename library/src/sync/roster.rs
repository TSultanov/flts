//! App-managed device roster — the mesh membership list.
//!
//! Stored as immutable operation rows in its own dbsync store: the shards in
//! `<library_root>/.flts/roster` sync as ordinary library content, the cache
//! stays device-local. Every node lists *all* mesh devices (including itself);
//! a peer that adopts the rows reconciles them against its own engine, so
//! adding a device on any node propagates everywhere (see [`super::reconcile`]).
//!
//! `.flts/devices.json` files, which older versions synced instead, are
//! imported on every [`RosterStore::load`] and never written.
//!
//! ## Causal merge (remove-wins)
//!
//! Membership is a **vector-clock CRDT**, not wall-clock last-writer-wins. Each
//! device id carries an *add context* and a *remove context* (both [`VClock`]s).
//! A local add/remove on this node bumps this node's component of the roster's
//! causal context and stamps the operation with it. The roster is the fold of
//! every operation: contexts join pointwise (a semilattice → commutative,
//! associative, idempotent), and a device is **present iff its add context
//! strictly dominates its remove context**. This is *remove-wins*: a removal
//! that causally follows the add it observed wins regardless of wall-clock
//! skew, and a concurrent removal wins because it carries a component the adds
//! cannot cover. A re-add observing the tombstone dominates it, so the device
//! comes back.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::Mutex,
};

use anyhow::Result;
use dbsync::{
    Id, WriterId,
    schema::{Layout, Store, Table},
};
use log::warn;
use serde::{Deserialize, Serialize};

mod schema {
    /// One immutable add or remove.
    #[dbsync::entity]
    pub struct Op {
        device_id: String,
        removed: bool,
        /// Empty on a removal.
        name: String,
        at_ms: u64,
        /// Canonical JSON of the operation's vector clock.
        clock: String,
    }

    /// A device saying which store writer it is. A device that lost its
    /// local state claims a new one; the newest claim is its writer.
    #[dbsync::entity]
    pub struct Claim {
        device_id: String,
        writer: u64,
        at_ms: u64,
    }

    #[dbsync::container(schema = "flts-roster", version = 1)]
    pub struct RosterDb {
        ops: Set<Op>,
        claims: Set<Claim>,
    }
}

use schema::RosterDb;

/// A version vector: `deviceId -> logical counter`. Missing keys are 0.
pub type VClock = BTreeMap<String, u64>;

/// `a` dominates `b`: `a[k] >= b[k]` for every component (missing = 0).
fn vc_dominates(a: &VClock, b: &VClock) -> bool {
    b.iter()
        .all(|(k, vb)| a.get(k).copied().unwrap_or(0) >= *vb)
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

/// One device in the derived compatibility view.
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

/// The full mesh membership. `adds`/`removes` are authoritative (causal);
/// `devices`/`removed` are a derived view, and all the oldest legacy files
/// carry. Every field defaults so legacy files of either schema parse.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Roster {
    /// Derived view: currently-present devices.
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceRecord>,
    /// Derived view: tombstoned devices `deviceId -> removed_at_ms`.
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
    /// Member iff its add context strictly dominates its remove context
    /// (remove-wins on concurrency). Two empty contexts mean an old-schema
    /// record, which keeps wall-clock meaning until some node re-stamps it.
    pub fn is_present(&self, id: &str) -> bool {
        match (self.adds.get(id), self.removes.get(id)) {
            (Some(a), Some(r)) => {
                if a.vc.is_empty() && r.vc.is_empty() {
                    a.added_at_ms >= r.removed_at_ms // old-schema LWW
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

    /// Seed `adds`/`removes` from the `devices`/`removed` view when a file
    /// carries only that, then rebuild the derived view. Idempotent.
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
        // Canonicalize so comparisons don't depend on the encoding.
        for a in self.adds.values_mut() {
            vc_canon(&mut a.vc);
        }
        for r in self.removes.values_mut() {
            vc_canon(&mut r.vc);
        }
        self.rebuild_view();
    }

    /// Recompute the derived view from `adds`/`removes` and the presence rule.
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

    /// Union-merge: per device, join both contexts pointwise and recompute
    /// presence. Commutative, associative, idempotent (a semilattice).
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

/// Join two add stamps. Display-only `name`/`added_at_ms` resolve by plain max:
/// a join can synthesize a clock no single op holds, so a domination-based pick
/// would not be associative.
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

/// One add or remove, as stored. Immutable.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RosterOp {
    pub device_id: String,
    pub removed: bool,
    /// Empty on a removal.
    pub name: String,
    pub at_ms: u64,
    pub vc: VClock,
}

impl RosterOp {
    /// One spelling per clock: sorted keys, no zero components.
    pub fn clock_json(&self) -> String {
        let mut vc = self.vc.clone();
        vc_canon(&mut vc);
        serde_json::to_string(&vc).unwrap_or_else(|_| "{}".into())
    }
}

impl Roster {
    /// The stamps as operations, one per add and per remove context.
    pub fn ops(&self) -> Vec<RosterOp> {
        let mut roster = self.clone();
        roster.normalize();
        let adds = roster.adds.into_iter().map(|(device_id, a)| RosterOp {
            device_id,
            removed: false,
            name: a.name,
            at_ms: a.added_at_ms,
            vc: a.vc,
        });
        let removes = roster.removes.into_iter().map(|(device_id, r)| RosterOp {
            device_id,
            removed: true,
            name: String::new(),
            at_ms: r.removed_at_ms,
            vc: r.vc,
        });
        adds.chain(removes).collect()
    }

    /// The roster a set of operations amounts to, whatever their order.
    ///
    /// Contexts are the join of all of a device's operations, as in
    /// [`Roster::merge`]. Name and time come only from operations no other one
    /// causally follows, so a rename replaces the name it observed.
    pub fn from_ops<'a>(ops: impl IntoIterator<Item = &'a RosterOp>) -> Roster {
        let mut groups: BTreeMap<(&str, bool), Vec<&RosterOp>> = BTreeMap::new();
        for op in ops {
            groups
                .entry((op.device_id.as_str(), op.removed))
                .or_default()
                .push(op);
        }
        let mut roster = Roster::default();
        for ((device_id, removed), group) in groups {
            let latest = group.iter().filter(|op| {
                !group
                    .iter()
                    .any(|other| vc_strictly_dominates(&canon(&other.vc), &canon(&op.vc)))
            });
            if removed {
                let stamp = latest.fold(None, |held: Option<RemStamp>, op| {
                    let stamp = RemStamp {
                        removed_at_ms: op.at_ms,
                        vc: canon(&op.vc),
                    };
                    Some(join_rem(held.as_ref(), Some(&stamp)))
                });
                roster
                    .removes
                    .insert(device_id.to_owned(), stamp.unwrap_or_default());
            } else {
                let stamp = latest.fold(None, |held: Option<AddStamp>, op| {
                    let stamp = AddStamp {
                        name: op.name.clone(),
                        added_at_ms: op.at_ms,
                        vc: canon(&op.vc),
                    };
                    Some(join_add(held.as_ref(), Some(&stamp)))
                });
                roster
                    .adds
                    .insert(device_id.to_owned(), stamp.unwrap_or_default());
            }
        }
        roster.rebuild_view();
        roster
    }
}

fn canon(vc: &VClock) -> VClock {
    let mut vc = vc.clone();
    vc_canon(&mut vc);
    vc
}

/// Namespace the operation row ids are derived under.
const ROOT: dbsync::Uuid = dbsync::Uuid::from_u128(0x666c_7473_726f_7374_6572_0000_0000_0001);

/// Shards, inside the library root so they sync.
const SHARED_DIR: &str = ".flts/roster";
/// Cache and lock, inside the library's device-local directory.
pub(crate) const LOCAL_DIR: &str = "roster";

struct Open {
    db: RosterDb,
    writer: WriterId,
    unpublished: bool,
}

impl Open {
    fn open(root: &Path) -> Result<Self> {
        let local = crate::store::local_dir(root);
        // The library store's writer: one identity per installation.
        let writer = crate::store::writer_id(&local)?;
        let (shared, local) = (root.join(SHARED_DIR), local.join(LOCAL_DIR));
        std::fs::create_dir_all(&shared)?;
        std::fs::create_dir_all(&local)?;
        let layout = Layout::split(&shared, &local);
        let db = RosterDb::open_or_create(layout.clone(), writer).or_else(|err| {
            // Only the cache can be unusable here; the shards rebuild it.
            if !err.is_corrupt() {
                return Err(err);
            }
            warn!("rebuilding roster cache: {err}");
            let _ = std::fs::remove_file(layout.cache(RosterDb::SCHEMA_VERSION));
            RosterDb::open_or_create(layout.clone(), writer)
        })?;
        Ok(Self {
            db,
            writer,
            unpublished: false,
        })
    }

    fn ops(&self) -> Result<Vec<RosterOp>> {
        let mut ops = Vec::new();
        for row in self.db.all::<schema::Op>() {
            let Ok(vc) = serde_json::from_str(&row.clock()?) else {
                warn!("roster: skipping an operation with an unreadable clock");
                continue;
            };
            ops.push(RosterOp {
                device_id: row.device_id()?.into_owned(),
                removed: row.removed()?,
                name: row.name()?.into_owned(),
                at_ms: row.at_ms()?,
                vc,
            });
        }
        Ok(ops)
    }

    /// The id is the operation's content, so writing it twice, here or on
    /// another device, is one row.
    fn stage(&mut self, op: &RosterOp) -> Result<()> {
        let clock = op.clock_json();
        let kind = if op.removed { "remove" } else { "add" };
        let key = serde_json::to_string(&(&op.device_id, kind, &clock, &op.name, op.at_ms))?;
        let id: Id<schema::Op> = Id::derived(ROOT, key.as_bytes());
        if self.db.ops.versions_of(id)?.is_empty() {
            self.unpublished = true;
            self.db
                .ops
                .insert(id, &op.device_id, op.removed, &op.name, op.at_ms, &clock)?;
        }
        Ok(())
    }

    fn claim(&mut self, device_id: &str) -> Result<()> {
        let writer = self.writer.raw();
        let key = format!("claim\0{device_id}\0{writer:016x}");
        let id: Id<schema::Claim> = Id::derived(ROOT, key.as_bytes());
        if self.db.claims.versions_of(id)?.is_empty() {
            self.unpublished = true;
            self.db.claims.insert(id, device_id, writer, now_ms())?;
            return Ok(());
        }
        // Back on an earlier writer, as after a restore: it is the current one again.
        let mine = self.db.claims.at_ms(id)?;
        let mut newer = false;
        for row in self.db.all::<schema::Claim>() {
            newer |= row.device_id()? == device_id && row.at_ms()? > mine;
        }
        if newer {
            self.unpublished = true;
            self.db.claims.set_at_ms(id, now_ms().max(mine + 1))?;
        }
        Ok(())
    }

    /// Writers no device in `roster` writes with any more: those of devices
    /// that left, and the older ones of a device that claimed anew.
    fn retired_writers(&self, roster: &Roster) -> Result<Vec<u64>> {
        let mut newest: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        let mut claimed = BTreeSet::new();
        for row in self.db.all::<schema::Claim>() {
            let (device, writer, at) = (row.device_id()?.into_owned(), row.writer()?, row.at_ms()?);
            claimed.insert(writer);
            let held = newest.entry(device).or_insert((at, writer));
            if (at, writer) > *held {
                *held = (at, writer);
            }
        }
        for (device, (_, writer)) in &newest {
            if roster.is_present(device) {
                claimed.remove(writer);
            }
        }
        claimed.remove(&self.writer.raw());
        Ok(claimed.into_iter().collect())
    }

    fn commit(&mut self) -> Result<()> {
        if self.unpublished {
            self.db.commit(self.writer)?;
            self.unpublished = false;
        }
        Ok(())
    }

    /// Takes in what other devices delivered: their shards, and the roster
    /// files of devices that still write those.
    fn adopt(&mut self, root: &Path) -> Result<()> {
        self.commit()?;
        // A copy of this installation writes the same shards; what it
        // replaced of ours goes back in. The library store sees to the rest.
        self.db.check_own_shards(self.writer)?;
        let report = self.db.refresh_peers()?;
        for path in &report.incompatible {
            warn!(
                "roster shard from another schema version skipped: {}",
                path.display()
            );
        }
        let files = legacy_files(root);
        if files.is_empty() {
            return Ok(());
        }
        let mut unreadable = Vec::new();
        for path in &files {
            match read_legacy(path) {
                Some(roster) => {
                    for op in roster.ops() {
                        self.stage(&op)?;
                    }
                }
                None => {
                    warn!("roster: ignoring unparseable {path:?}");
                    unreadable.push(path);
                }
            }
        }
        self.commit()?;
        let backup = crate::store::local_dir(root).join(crate::legacy::BACKUP_DIR);
        for path in &files {
            if let Err(err) = crate::legacy::set_aside(path, &backup, unreadable.contains(&path)) {
                warn!("roster: {path:?} was imported but could not be set aside: {err}");
            }
        }
        Ok(())
    }
}

/// `.flts/devices.json` and its `devices.sync-conflict-*.json` siblings.
fn legacy_files(root: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = std::fs::read_dir(root.join(".flts"))
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name == "devices.json"
                || (name.starts_with("devices.sync-conflict-") && name.ends_with(".json"))
        })
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect();
    found.sort();
    found
}

fn read_legacy(path: &Path) -> Option<Roster> {
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

/// Reads the roster from its store and appends operations to it.
pub struct RosterStore {
    root: PathBuf,
    /// This node's device id — the vector-clock component a local op increments.
    node_id: String,
    /// Opened on first use and kept: a second handle would not see this
    /// one's later commits.
    db: Mutex<Option<Open>>,
    #[cfg(all(feature = "tla_trace", feature = "sync-engine"))]
    delivered_from: Mutex<Vec<String>>,
}

impl RosterStore {
    /// Roster of the library at `library_root`, owned by `node_id` (this
    /// device's Syncthing id — the vector-clock component local ops advance).
    pub fn new(library_root: &Path, node_id: &str) -> Self {
        Self {
            root: library_root.to_owned(),
            node_id: node_id.to_string(),
            db: Mutex::new(None),
            #[cfg(all(feature = "tla_trace", feature = "sync-engine"))]
            delivered_from: Mutex::new(Vec::new()),
        }
    }

    fn with<T>(&self, f: impl FnOnce(&mut Open) -> Result<T>) -> Result<T> {
        let mut held = self.db.lock().unwrap_or_else(|e| e.into_inner());
        let open = match &mut *held {
            Some(open) => open,
            None => held.insert(Open::open(&self.root)?),
        };
        f(open)
    }

    /// Adopts what other devices delivered, then folds every operation.
    pub fn load(&self) -> Result<Roster> {
        self.with(|db| {
            db.adopt(&self.root)?;
            Ok(Roster::from_ops(&db.ops()?))
        })
    }

    /// Appends one operation stamped to dominate everything seen: this node's
    /// component bumped over the join of all contexts. One lock, so two local
    /// operations cannot share a clock.
    fn append(&self, op: impl FnOnce(VClock) -> RosterOp) -> Result<Roster> {
        self.with(|db| {
            db.adopt(&self.root)?;
            let mut ops = db.ops()?;
            let mut vc = Roster::from_ops(&ops).context();
            *vc.entry(self.node_id.clone()).or_insert(0) += 1;
            let op = op(vc);
            db.stage(&op)?;
            db.commit()?;
            ops.push(op);
            Ok(Roster::from_ops(&ops))
        })
    }

    /// Adds or refreshes a device; the add dominates any tombstone it observed.
    pub fn add_device(&self, device_id: &str, name: &str) -> Result<Roster> {
        self.append(|vc| RosterOp {
            device_id: device_id.to_string(),
            removed: false,
            name: name.to_string(),
            at_ms: now_ms(),
            vc,
        })
    }

    /// Tombstones a device; the removal dominates the add it observed.
    pub fn remove_device(&self, device_id: &str) -> Result<Roster> {
        self.append(|vc| RosterOp {
            device_id: device_id.to_string(),
            removed: true,
            name: String::new(),
            at_ms: now_ms(),
            vc,
        })
    }

    /// See [`Open::retired_writers`].
    pub fn retired_writers(&self) -> Result<Vec<u64>> {
        self.with(|db| {
            db.adopt(&self.root)?;
            db.retired_writers(&Roster::from_ops(&db.ops()?))
        })
    }

    /// Ensures this device is listed so peers learn about it, and which
    /// store writer it is.
    pub fn ensure_self(&self, my_id: &str, name: &str) -> Result<Roster> {
        self.with(|db| {
            db.claim(my_id)?;
            db.commit()
        })?;
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

    /// The roster as last adopted, so the engine can tell whether a `load`
    /// took anything in. Trace-only.
    #[cfg(all(feature = "tla_trace", feature = "sync-engine"))]
    pub(crate) fn snapshot_for_trace(&self) -> Roster {
        self.with(|db| Ok(Roster::from_ops(&db.ops()?)))
            .unwrap_or_default()
    }

    /// Shards are named by writer, not device id, so whoever delivers them
    /// names the source device. Trace-only.
    #[cfg(all(test, feature = "tla_trace", feature = "sync-engine"))]
    pub(crate) fn note_delivery_from(&self, src: &str) {
        self.delivered_from.lock().unwrap().push(src.to_string());
    }

    /// Sources noted since the last call. Trace-only.
    #[cfg(all(feature = "tla_trace", feature = "sync-engine"))]
    pub(crate) fn take_delivery_sources(&self) -> Vec<String> {
        std::mem::take(&mut self.delivered_from.lock().unwrap())
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
    use crate::test_utils::{TempDir, copy_shards};
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
        let a = one("A", Some(add("a", 1, vc(&[("A", 1)]))), None).merge(&one(
            "B",
            Some(add("b", 1, vc(&[("B", 1)]))),
            None,
        ));
        let merged = a.merge(&one("C", Some(add("c", 1, vc(&[("C", 1)]))), None));
        assert_eq!(
            merged.devices.keys().collect::<Vec<_>>(),
            vec!["A", "B", "C"]
        );
        assert!(merged.removed.is_empty());
    }

    #[test]
    fn dominant_remove_wins_despite_clock_skew() {
        let added = one("X", Some(add("x", 100, vc(&[("A", 1)]))), None);
        // B observed the add then removed X, but its wall clock lags.
        let removed = one("X", None, Some(rem(0, vc(&[("A", 1), ("B", 1)]))));

        let ab = added.merge(&removed);
        let ba = removed.merge(&added);
        assert_eq!(ab, ba, "merge is commutative");
        assert!(
            !ab.is_present("X"),
            "causally-later removal wins under skew"
        );
        assert!(ab.removed.contains_key("X"));
    }

    #[test]
    fn concurrent_add_and_remove_remove_wins() {
        let added = one("X", Some(add("x", 5, vc(&[("A", 1)]))), None);
        let removed = one("X", None, Some(rem(5, vc(&[("C", 1)]))));
        let m = added.merge(&removed);
        assert!(!m.is_present("X"), "remove-wins on concurrency");
    }

    #[test]
    fn re_add_after_remove_resurrects() {
        let removed = one(
            "X",
            Some(add("x", 1, vc(&[("A", 1)]))),
            Some(rem(2, vc(&[("A", 1), ("B", 1)]))),
        );
        assert!(!removed.is_present("X"));
        let readd = one("X", Some(add("x", 3, vc(&[("A", 1), ("B", 2)]))), None);
        let m = removed.merge(&readd);
        assert!(m.is_present("X"), "causally-later re-add resurrects");
    }

    fn root(dir: &TempDir, device: &str) -> PathBuf {
        dir.path.join(device).join("library")
    }

    /// Delivers `from`'s roster shards to `to`, and nothing else.
    fn deliver(from: &Path, to: &Path) {
        copy_shards(&from.join(SHARED_DIR), &to.join(SHARED_DIR));
    }

    fn rows(store: &RosterStore) -> Vec<RosterOp> {
        let mut ops = store.with(|db| db.ops()).unwrap();
        ops.sort();
        ops
    }

    fn backup(root: &Path) -> PathBuf {
        let backup = crate::store::local_dir(root).join(crate::legacy::BACKUP_DIR);
        crate::legacy::bucket(&backup, false)
    }

    fn write_legacy(root: &Path, name: &str, contents: &str) {
        std::fs::create_dir_all(root.join(".flts")).unwrap();
        std::fs::write(root.join(".flts").join(name), contents).unwrap();
    }

    fn writer_of(root: &Path) -> u64 {
        crate::store::writer_id(&crate::store::local_dir(root))
            .unwrap()
            .raw()
    }

    #[test]
    fn a_writer_is_retired_once_its_device_left_or_claimed_another() {
        let dir = TempDir::new("flts_roster");
        let (ra, rb) = (root(&dir, "a"), root(&dir, "b"));
        let (a, b) = (RosterStore::new(&ra, "A"), RosterStore::new(&rb, "B"));
        a.ensure_self("A", "alpha").unwrap();
        b.ensure_self("B", "beta").unwrap();
        a.add_device("B", "beta").unwrap();
        deliver(&ra, &rb);
        deliver(&rb, &ra);
        assert!(a.retired_writers().unwrap().is_empty());
        let first = writer_of(&rb);

        // B loses its local state and comes back as a new writer.
        drop(b);
        std::fs::remove_dir_all(crate::store::local_dir(&rb)).unwrap();
        let b = RosterStore::new(&rb, "B");
        b.ensure_self("B", "beta").unwrap();
        deliver(&rb, &ra);
        assert_eq!(a.retired_writers().unwrap(), [first]);

        a.remove_device("B").unwrap();
        let mut retired = a.retired_writers().unwrap();
        retired.sort_unstable();
        let mut both = [first, writer_of(&rb)];
        both.sort_unstable();
        assert_eq!(retired, both);
        assert!(!retired.contains(&writer_of(&ra)));
    }

    #[test]
    fn two_copies_of_one_installation_keep_each_others_roster_changes() {
        let dir = TempDir::new("flts_roster");
        let (ra, rb) = (root(&dir, "a"), root(&dir, "b"));
        RosterStore::new(&ra, "A")
            .ensure_self("A", "alpha")
            .unwrap();
        // The whole installation copied, local state included.
        crate::test_utils::copy_shards(&dir.path.join("a"), &dir.path.join("b"));
        assert_eq!(writer_of(&ra), writer_of(&rb));
        let (a, b) = (RosterStore::new(&ra, "A"), RosterStore::new(&rb, "A"));
        a.add_device("B", "beta").unwrap();
        b.add_device("C", "gamma").unwrap();

        deliver(&ra, &rb);
        let seen_b = b.load().unwrap();
        deliver(&rb, &ra);
        let seen_a = a.load().unwrap();
        assert_eq!(seen_a, seen_b);
        assert_eq!(seen_a.devices.keys().collect::<Vec<_>>(), ["A", "B", "C"]);
    }

    #[test]
    fn two_stores_exchanging_only_roster_shards_converge() {
        let dir = TempDir::new("flts_roster");
        let (ra, rb) = (root(&dir, "a"), root(&dir, "b"));
        let (a, b) = (RosterStore::new(&ra, "A"), RosterStore::new(&rb, "B"));
        a.ensure_self("A", "alpha").unwrap();
        b.ensure_self("B", "beta").unwrap();
        a.add_device("B", "beta").unwrap();
        b.add_device("C", "gamma").unwrap();
        b.remove_device("C").unwrap();

        deliver(&ra, &rb);
        deliver(&rb, &ra);
        let (seen_a, seen_b) = (a.load().unwrap(), b.load().unwrap());
        assert_eq!(seen_a, seen_b);
        assert_eq!(seen_a.devices.keys().collect::<Vec<_>>(), ["A", "B"]);
        assert!(seen_a.removed.contains_key("C"));
        assert_eq!(rows(&a), rows(&b));

        drop(a);
        let reopened = RosterStore::new(&ra, "A");
        assert_eq!(reopened.load().unwrap(), seen_a, "survives a reopen");
        assert!(
            !ra.join(".flts/devices.json").exists(),
            "nothing writes the roster file"
        );
    }

    #[test]
    fn concurrent_removal_on_another_device_wins_on_both() {
        let dir = TempDir::new("flts_roster");
        let (ra, rb) = (root(&dir, "a"), root(&dir, "b"));
        let (a, b) = (RosterStore::new(&ra, "A"), RosterStore::new(&rb, "B"));
        a.add_device("X", "x").unwrap();
        deliver(&ra, &rb);
        // Concurrently: A refreshes X, B removes it.
        a.add_device("X", "x again").unwrap();
        b.remove_device("X").unwrap();
        deliver(&ra, &rb);
        deliver(&rb, &ra);

        let (seen_a, seen_b) = (a.load().unwrap(), b.load().unwrap());
        assert_eq!(seen_a, seen_b, "both devices fold the same rows");
        assert!(!seen_a.is_present("X"), "remove-wins on concurrency");

        // A re-add that observed the tombstone brings it back everywhere.
        b.add_device("X", "x").unwrap();
        deliver(&rb, &ra);
        assert!(a.load().unwrap().is_present("X"));
    }

    #[test]
    fn the_same_operation_written_twice_is_one_row() {
        let dir = TempDir::new("flts_roster");
        let (ra, rb) = (root(&dir, "a"), root(&dir, "b"));
        let (a, b) = (RosterStore::new(&ra, "A"), RosterStore::new(&rb, "B"));
        let op = RosterOp {
            device_id: "X".into(),
            removed: false,
            name: "x".into(),
            at_ms: 7,
            vc: vc(&[("A", 1), ("Z", 0)]),
        };
        for store in [&a, &b, &a] {
            store
                .with(|db| {
                    db.stage(&op)?;
                    db.commit()
                })
                .unwrap();
        }
        deliver(&ra, &rb);
        deliver(&rb, &ra);
        for store in [&a, &b] {
            store.load().unwrap();
            let ops = rows(store);
            assert_eq!(ops.len(), 1);
            assert_eq!(ops[0].vc, vc(&[("A", 1)]), "stored canonically");
        }
    }

    #[test]
    fn a_rename_replaces_the_name_it_observed() {
        let dir = TempDir::new("flts_roster");
        let store = RosterStore::new(&root(&dir, "a"), "SELF");
        store.ensure_self("SELF", "Zed").unwrap();
        let renamed = store.ensure_self("SELF", "Alpha").unwrap();
        assert_eq!(renamed.devices["SELF"].name, "Alpha");
        assert_eq!(store.load().unwrap(), renamed);

        let before = rows(&store).len();
        store.ensure_self("SELF", "Alpha").unwrap();
        assert_eq!(
            rows(&store).len(),
            before,
            "an unchanged name writes nothing"
        );
    }

    #[test]
    fn add_remove_readd_via_store_no_sleep() {
        let dir = TempDir::new("flts_roster");
        let store = RosterStore::new(&root(&dir, "a"), "SELF");

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
    }

    // ---- Legacy roster files ----

    const OLD_SCHEMA: &str =
        r#"{"devices":{"A":{"name":"alpha","addedAtMs":10}},"removed":{"B":5}}"#;

    #[test]
    fn an_old_schema_file_is_imported_and_set_aside() {
        let dir = TempDir::new("flts_roster");
        let ra = root(&dir, "a");
        write_legacy(&ra, "devices.json", OLD_SCHEMA);
        let store = RosterStore::new(&ra, "SELF");

        let roster = store.load().unwrap();
        assert_eq!(roster.devices["A"].name, "alpha");
        assert_eq!(roster.devices["A"].added_at_ms, 10);
        assert_eq!(roster.removed["B"], 5);
        assert!(roster.adds["A"].vc.is_empty(), "unstamped until re-stamped");

        assert!(!ra.join(".flts/devices.json").exists());
        assert!(ra.join(SHARED_DIR).is_dir(), "`.flts` itself stays");
        assert_eq!(
            std::fs::read_to_string(backup(&ra).join("devices.json")).unwrap(),
            OLD_SCHEMA
        );
        assert_eq!(store.load().unwrap(), roster);
    }

    #[test]
    fn a_siblings_concurrent_removal_wins_on_import() {
        let dir = TempDir::new("flts_roster");
        let ra = root(&dir, "a");
        let mine = one("X", Some(add("x", 5, vc(&[("A", 1)]))), None).merge(&one(
            "A",
            Some(add("alpha", 1, vc(&[("A", 2)]))),
            None,
        ));
        let theirs = one("X", None, Some(rem(9, vc(&[("C", 1)]))));
        write_legacy(&ra, "devices.json", &serde_json::to_string(&mine).unwrap());
        write_legacy(
            &ra,
            "devices.sync-conflict-20260530-120000-OTHER.json",
            &serde_json::to_string(&theirs).unwrap(),
        );
        write_legacy(&ra, "devices.sync-conflict-20260530-120001-BAD.json", "{");

        let roster = RosterStore::new(&ra, "A").load().unwrap();
        assert_eq!(roster, mine.merge(&theirs), "the old loader's union-merge");
        assert!(!roster.is_present("X"), "remove-wins on concurrency");
        assert!(roster.is_present("A"));
        assert!(legacy_files(&ra).is_empty(), "siblings are set aside too");
        assert_eq!(std::fs::read_dir(backup(&ra)).unwrap().count(), 2);
        let kept = crate::store::local_dir(&ra).join(crate::legacy::BACKUP_DIR);
        assert!(
            crate::legacy::bucket(&kept, true)
                .join("devices.sync-conflict-20260530-120001-BAD.json")
                .is_file(),
            "what could not be parsed is kept for good"
        );
    }

    #[test]
    fn a_file_delivered_again_adds_no_rows() {
        let dir = TempDir::new("flts_roster");
        let ra = root(&dir, "a");
        let store = RosterStore::new(&ra, "SELF");
        write_legacy(&ra, "devices.json", OLD_SCHEMA);
        let first = store.load().unwrap();
        let held = rows(&store);
        assert_eq!(held.len(), 2);

        // A device still on the old format syncs its file back in.
        write_legacy(&ra, "devices.json", OLD_SCHEMA);
        assert_eq!(store.load().unwrap(), first);
        assert_eq!(rows(&store), held);
        assert!(legacy_files(&ra).is_empty());
        assert!(backup(&ra).join("devices.json.1").exists());

        // And later with something new in it.
        write_legacy(
            &ra,
            "devices.json",
            r#"{"devices":{"A":{"name":"alpha","addedAtMs":10},"C":{"name":"c","addedAtMs":11}}}"#,
        );
        let grown = store.load().unwrap();
        assert!(grown.is_present("C"));
        assert_eq!(rows(&store).len(), 3);
    }

    #[test]
    fn two_devices_importing_the_same_file_converge() {
        let dir = TempDir::new("flts_roster");
        let (ra, rb) = (root(&dir, "a"), root(&dir, "b"));
        let file = serde_json::to_string(
            &one("X", Some(add("x", 5, vc(&[("A", 1)]))), None).merge(&one(
                "Y",
                None,
                Some(rem(6, vc(&[("B", 2)]))),
            )),
        )
        .unwrap();
        write_legacy(&ra, "devices.json", &file);
        write_legacy(&rb, "devices.json", &file);
        let (a, b) = (RosterStore::new(&ra, "A"), RosterStore::new(&rb, "B"));
        let (alone_a, alone_b) = (a.load().unwrap(), b.load().unwrap());
        assert_eq!(alone_a, alone_b);
        let imported = rows(&a);

        deliver(&ra, &rb);
        deliver(&rb, &ra);
        for store in [&a, &b] {
            assert_eq!(store.load().unwrap(), alone_a);
            assert_eq!(
                rows(store),
                imported,
                "one row per operation, not per device"
            );
        }
    }

    // ---- Mixed-schema compatibility ----

    #[test]
    fn legacy_json_deserializes_and_normalizes() {
        let legacy = r#"{"devices":{"A":{"name":"alpha","addedAtMs":10}},"removed":{"B":5}}"#;
        let mut roster: Roster = serde_json::from_str(legacy).unwrap();
        roster.normalize();
        assert!(roster.is_present("A"), "legacy active device stays present");
        assert!(!roster.is_present("B"), "legacy tombstone stays removed");
        assert!(
            roster.adds["A"].vc.is_empty(),
            "seeded with empty (legacy) vc"
        );
    }

    #[test]
    fn legacy_add_loses_to_new_remove() {
        // Add without a vc vs a remove carrying one.
        let legacy_add = one("X", Some(add("x", 100, VClock::new())), None);
        let new_remove = one("X", None, Some(rem(0, vc(&[("B", 1)]))));
        assert!(!legacy_add.merge(&new_remove).is_present("X"));
    }

    #[test]
    fn new_node_op_restamps_legacy_entry() {
        // An op over an unstamped entry self-heals it with a vc.
        let dir = TempDir::new("flts_roster");
        let ra = root(&dir, "a");
        write_legacy(
            &ra,
            "devices.json",
            r#"{"devices":{"P":{"name":"peer","addedAtMs":1}},"removed":{}}"#,
        );
        let store = RosterStore::new(&ra, "SELF");
        store.remove_device("P").unwrap();
        let r = store.load().unwrap();
        assert!(!r.is_present("P"));
        assert!(
            !r.removes["P"].vc.is_empty(),
            "removal carries a real vc now"
        );
    }

    // ---- Convergence ----

    fn arb_vc() -> impl Strategy<Value = VClock> {
        prop::collection::btree_map("[A-C]", 0u64..3, 0..3)
    }
    fn arb_roster() -> impl Strategy<Value = Roster> {
        let adds = prop::collection::btree_map(
            "[X-Z]",
            (0u64..3, arb_vc()).prop_map(|(ts, v)| add("n", ts, v)),
            0..3,
        );
        let removes = prop::collection::btree_map(
            "[X-Z]",
            (0u64..3, arb_vc()).prop_map(|(ts, v)| rem(ts, v)),
            0..3,
        );
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

    /// Every operation joined, superseded or not: what merging files did.
    fn join_all(ops: &BTreeSet<RosterOp>) -> Roster {
        ops.iter().fold(Roster::default(), |held, op| {
            held.merge(&Roster::from_ops([op]))
        })
    }

    fn contexts(roster: &Roster) -> (Vec<(&String, &VClock)>, Vec<(&String, &VClock)>) {
        (
            roster.adds.iter().map(|(id, a)| (id, &a.vc)).collect(),
            roster.removes.iter().map(|(id, r)| (id, &r.vc)).collect(),
        )
    }

    fn superseded(op: &RosterOp, ops: &BTreeSet<RosterOp>) -> bool {
        ops.iter().any(|other| {
            (&other.device_id, other.removed) == (&op.device_id, op.removed)
                && vc_strictly_dominates(&other.vc, &op.vc)
        })
    }

    proptest! {
        /// Row union + fold is the old file merge.
        #[test]
        fn folding_the_union_of_rows_is_merging_the_rosters(
            rosters in prop::collection::vec(arb_roster(), 0..5),
            order in any::<prop::sample::Index>(),
        ) {
            let merged = rosters
                .iter()
                .fold(Roster::default(), |held, roster| held.merge(roster));
            let ops: BTreeSet<RosterOp> = rosters.iter().flat_map(Roster::ops).collect();
            prop_assert_eq!(&join_all(&ops), &merged);

            // Any arrival order, duplicates included, folds the same.
            let mut shuffled: Vec<&RosterOp> = ops.iter().chain(ops.iter()).collect();
            if !shuffled.is_empty() {
                let at = order.index(shuffled.len());
                shuffled.rotate_left(at);
            }
            let folded = Roster::from_ops(shuffled);
            prop_assert_eq!(&folded, &Roster::from_ops(&ops));

            // Membership is exactly the merge's; only display metadata of
            // superseded operations is dropped.
            prop_assert_eq!(contexts(&folded), contexts(&merged));
            prop_assert_eq!(
                folded.devices.keys().collect::<Vec<_>>(),
                merged.devices.keys().collect::<Vec<_>>()
            );
            prop_assert_eq!(
                folded.removed.keys().collect::<Vec<_>>(),
                merged.removed.keys().collect::<Vec<_>>()
            );
            if !ops.iter().any(|op| superseded(op, &ops)) {
                prop_assert_eq!(&folded, &merged);
            }
        }

        #[test]
        fn a_roster_is_the_fold_of_its_own_operations(a in arb_roster()) {
            prop_assert_eq!(Roster::from_ops(&a.ops()), a.merge(&Roster::default()));
        }

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
