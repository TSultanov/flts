//! Pure reconciliation: diff the roster against the engine's device set.
//!
//! Kept side-effect-free so it is exhaustively unit-testable; the engine applies
//! the resulting plan ([`super::engine::SyncEngine::reconcile_once`]).

use std::collections::BTreeSet;

use super::roster::Roster;

/// What the engine must do to match the roster.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ReconcilePlan {
    /// `(device_id, name)` to add + share the folder with.
    pub to_add: Vec<(String, String)>,
    /// Device IDs to remove (opt-in: only roster-tombstoned devices).
    pub to_remove: Vec<String>,
}

impl ReconcilePlan {
    pub fn is_empty(&self) -> bool {
        self.to_add.is_empty() && self.to_remove.is_empty()
    }
}

/// Diffs `roster` against the engine's current `engine_ids`.
///
/// - **add**: active roster devices missing from the engine (never self).
/// - **remove**: engine devices the roster *tombstones* and does not re-list
///   (opt-in removal; never self). A device merely absent from the roster is
///   left alone, so a peer that hasn't yet learned of it isn't torn down.
pub fn reconcile(roster: &Roster, engine_ids: &BTreeSet<String>, my_id: &str) -> ReconcilePlan {
    let mut to_add = Vec::new();
    for (id, rec) in &roster.devices {
        if id == my_id {
            continue;
        }
        if !engine_ids.contains(id) {
            to_add.push((id.clone(), rec.name.clone()));
        }
    }

    let mut to_remove = Vec::new();
    for id in engine_ids {
        if id == my_id {
            continue;
        }
        if roster.removed.contains_key(id) && !roster.devices.contains_key(id) {
            to_remove.push(id.clone());
        }
    }

    ReconcilePlan { to_add, to_remove }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::roster::DeviceRecord;

    fn roster_with(active: &[(&str, u64)], removed: &[(&str, u64)]) -> Roster {
        let mut r = Roster::default();
        for (id, ts) in active {
            r.devices.insert(
                (*id).into(),
                DeviceRecord {
                    name: format!("name-{id}"),
                    added_at_ms: *ts,
                },
            );
        }
        for (id, ts) in removed {
            r.removed.insert((*id).into(), *ts);
        }
        r
    }

    fn ids(list: &[&str]) -> BTreeSet<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn adds_roster_devices_missing_from_engine() {
        let roster = roster_with(&[("SELF", 1), ("A", 1), ("B", 1)], &[]);
        let plan = reconcile(&roster, &ids(&["SELF"]), "SELF");
        let added: BTreeSet<_> = plan.to_add.iter().map(|(id, _)| id.clone()).collect();
        assert_eq!(added, ids(&["A", "B"]));
        assert!(plan.to_remove.is_empty());
    }

    #[test]
    fn never_adds_or_removes_self() {
        let roster = roster_with(&[("SELF", 1)], &[("SELF", 9)]);
        let plan = reconcile(&roster, &ids(&["SELF"]), "SELF");
        assert!(plan.is_empty());
    }

    #[test]
    fn removes_only_tombstoned_engine_devices() {
        // C is tombstoned; D is in the engine but simply absent from the roster.
        let roster = roster_with(&[("SELF", 1)], &[("C", 5)]);
        let plan = reconcile(&roster, &ids(&["SELF", "C", "D"]), "SELF");
        assert_eq!(plan.to_remove, vec!["C".to_string()]);
        assert!(plan.to_add.is_empty(), "D absent-from-roster is left alone");
    }

    #[test]
    fn no_op_when_engine_matches_roster() {
        let roster = roster_with(&[("SELF", 1), ("A", 1)], &[]);
        let plan = reconcile(&roster, &ids(&["SELF", "A"]), "SELF");
        assert!(plan.is_empty());
    }
}
