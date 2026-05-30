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
//! ([`crate::library::library_card`]). Merge is last-writer-wins per device
//! between an add (`added_at_ms`) and a removal tombstone (`removed`), so it is
//! order-independent and convergent.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use log::warn;
use serde::{Deserialize, Serialize};

/// One device in the roster.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct DeviceRecord {
    pub name: String,
    #[serde(rename = "addedAtMs")]
    pub added_at_ms: u64,
}

/// The full mesh membership: active devices + removal tombstones.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct Roster {
    #[serde(default)]
    pub devices: BTreeMap<String, DeviceRecord>,
    /// Tombstones: `deviceId -> removed_at_ms`. A tombstone wins over an add
    /// only if its timestamp is newer, so re-adding a removed device works.
    #[serde(default)]
    pub removed: BTreeMap<String, u64>,
}

impl Roster {
    /// Union-merge with last-writer-wins per device. Order-independent: merging
    /// any set of roster copies (canonical + conflict siblings) in any order
    /// yields the same result.
    pub fn merge(&self, other: &Roster) -> Roster {
        let mut ids: BTreeSet<&String> = BTreeSet::new();
        ids.extend(self.devices.keys());
        ids.extend(self.removed.keys());
        ids.extend(other.devices.keys());
        ids.extend(other.removed.keys());

        let mut merged = Roster::default();
        for id in ids {
            // Newest add and newest removal seen for this device, if any.
            let add = [self.devices.get(id), other.devices.get(id)]
                .into_iter()
                .flatten()
                .max_by_key(|r| r.added_at_ms);
            let removed_at = [self.removed.get(id), other.removed.get(id)]
                .into_iter()
                .flatten()
                .copied()
                .max();

            match (add, removed_at) {
                // Tombstone is strictly newer than the latest add → removed.
                (Some(rec), Some(rts)) if rts > rec.added_at_ms => {
                    merged.removed.insert(id.clone(), rts);
                }
                // An add at least as new as any tombstone → active.
                (Some(rec), _) => {
                    merged.devices.insert(id.clone(), rec.clone());
                }
                (None, Some(rts)) => {
                    merged.removed.insert(id.clone(), rts);
                }
                (None, None) => {}
            }
        }
        merged
    }
}

/// Reads, merges, and persists the roster file.
pub struct RosterStore {
    dir: PathBuf,
    path: PathBuf,
}

const ROSTER_BASENAME: &str = "devices";
const ROSTER_FILENAME: &str = "devices.json";

impl RosterStore {
    /// Roster under `<library_root>/.flts/devices.json`.
    pub fn new(library_root: &Path) -> Self {
        let dir = library_root.join(".flts");
        let path = dir.join(ROSTER_FILENAME);
        Self { dir, path }
    }

    /// Loads the roster, union-merging any `.sync-conflict-*` siblings and then
    /// writing the merged result back and deleting the siblings.
    pub fn load(&self) -> Result<Roster> {
        let mut roster = read_roster(&self.path).unwrap_or_default();

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

    /// Adds or refreshes a device (clearing any tombstone). Returns the saved
    /// roster.
    pub fn add_device(&self, device_id: &str, name: &str) -> Result<Roster> {
        let mut roster = self.load()?;
        roster.removed.remove(device_id);
        roster.devices.insert(
            device_id.to_string(),
            DeviceRecord {
                name: name.to_string(),
                added_at_ms: now_ms(),
            },
        );
        self.save(&roster)?;
        Ok(roster)
    }

    /// Tombstones a device (opt-in removal). Returns the saved roster.
    pub fn remove_device(&self, device_id: &str) -> Result<Roster> {
        let mut roster = self.load()?;
        roster.devices.remove(device_id);
        roster.removed.insert(device_id.to_string(), now_ms());
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

    fn rec(name: &str, ts: u64) -> DeviceRecord {
        DeviceRecord {
            name: name.to_string(),
            added_at_ms: ts,
        }
    }

    #[test]
    fn merge_unions_devices() {
        let mut a = Roster::default();
        a.devices.insert("A".into(), rec("a", 1));
        a.devices.insert("B".into(), rec("b", 1));
        let mut b = Roster::default();
        b.devices.insert("B".into(), rec("b", 1));
        b.devices.insert("C".into(), rec("c", 1));

        let merged = a.merge(&b);
        assert_eq!(
            merged.devices.keys().collect::<Vec<_>>(),
            vec!["A", "B", "C"]
        );
        assert!(merged.removed.is_empty());
    }

    #[test]
    fn merge_is_order_independent_and_lww() {
        // One side removed X at t=10; the other still has it added at t=5.
        let mut active = Roster::default();
        active.devices.insert("X".into(), rec("x", 5));
        let mut removed = Roster::default();
        removed.removed.insert("X".into(), 10);

        let ab = active.merge(&removed);
        let ba = removed.merge(&active);
        assert_eq!(ab, ba, "merge is commutative");
        assert!(!ab.devices.contains_key("X"), "newer tombstone wins");
        assert_eq!(ab.removed.get("X"), Some(&10));

        // Re-adding X at t=12 beats the t=10 tombstone.
        let mut readd = Roster::default();
        readd.devices.insert("X".into(), rec("x", 12));
        let resurrected = ab.merge(&readd);
        assert!(resurrected.devices.contains_key("X"));
        assert!(!resurrected.removed.contains_key("X"));
    }

    #[test]
    fn load_merges_and_clears_conflict_siblings() {
        let tmp = std::env::temp_dir().join(format!("flts-roster-{}", now_ms()));
        let store = RosterStore::new(&tmp);
        store.add_device("A", "alpha").unwrap();

        // Simulate a Syncthing conflict sibling with a different device.
        let mut other = Roster::default();
        other.devices.insert("B".into(), rec("beta", now_ms()));
        let sibling = tmp
            .join(".flts")
            .join("devices.sync-conflict-20260530-120000-ABCDEFG.json");
        fs::write(&sibling, serde_json::to_vec(&other).unwrap()).unwrap();

        let merged = store.load().unwrap();
        assert!(merged.devices.contains_key("A"));
        assert!(merged.devices.contains_key("B"), "sibling merged in");
        assert!(!sibling.exists(), "sibling cleaned up");
        // And the merge was persisted.
        assert!(store.load().unwrap().devices.contains_key("B"));

        let _ = fs::remove_dir_all(&tmp);
    }

    #[test]
    fn add_then_remove_then_readd() {
        let tmp = std::env::temp_dir().join(format!("flts-roster-rr-{}", now_ms()));
        let store = RosterStore::new(&tmp);

        store.add_device("P", "peer").unwrap();
        assert!(store.load().unwrap().devices.contains_key("P"));

        store.remove_device("P").unwrap();
        let r = store.load().unwrap();
        assert!(!r.devices.contains_key("P"));
        assert!(r.removed.contains_key("P"));

        // add_device must clear the tombstone with a fresh timestamp.
        std::thread::sleep(std::time::Duration::from_millis(2));
        store.add_device("P", "peer").unwrap();
        let r = store.load().unwrap();
        assert!(r.devices.contains_key("P"));
        assert!(!r.removed.contains_key("P"));

        let _ = fs::remove_dir_all(&tmp);
    }
}
