//! Append-only Skill load lifecycle reduction.

use std::collections::BTreeSet;

use crate::event::SkillLoadFact;

/// Reduced loaded-Skill state for one execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillLifecycleState {
    facts: Vec<SkillLoadFact>,
    loaded: Vec<SkillLoadFact>,
    loaded_ids: BTreeSet<String>,
}

impl SkillLifecycleState {
    /// Returns every append-only load fact in ledger order.
    pub fn facts(&self) -> &[SkillLoadFact] {
        &self.facts
    }

    /// Returns the first load of each Skill in ledger order.
    pub fn loaded(&self) -> &[SkillLoadFact] {
        &self.loaded
    }

    /// Returns false because durable loaded Skills have no unload command.
    pub fn can_unload(&self, _skill_id: &str) -> bool {
        false
    }

    /// Returns whether a device-local staged entry may be removed.
    pub fn can_remove_staged(&self, skill_id: &str, device_local: bool) -> bool {
        device_local && !self.loaded_ids.contains(skill_id)
    }
}

/// Reduces immutable Skill load facts into current loaded state.
pub struct SkillLifecycle;

impl SkillLifecycle {
    /// Reduces facts without inventing unload or origin mutations.
    pub fn reduce(facts: impl IntoIterator<Item = SkillLoadFact>) -> SkillLifecycleState {
        let facts = facts.into_iter().collect::<Vec<_>>();
        let mut loaded_ids = BTreeSet::new();
        let loaded = facts
            .iter()
            .filter(|fact| loaded_ids.insert(fact.skill_id.clone()))
            .cloned()
            .collect();
        SkillLifecycleState {
            facts,
            loaded,
            loaded_ids,
        }
    }
}
