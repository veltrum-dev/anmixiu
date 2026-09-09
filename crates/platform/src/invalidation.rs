use std::collections::{HashMap, HashSet};

use anmixiu_reactive::OwnerId;

/// One owner that reached the consecutive anonymous self-invalidation limit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RunawayInvalidation {
    pub owner: OwnerId,
    pub streak: usize,
}

/// Tracks anonymous self-invalidation independently for every live owner.
///
/// The key is an `OwnerId`. `advance` removes owners absent from the current turn, so a settled
/// turn resets only that owner. The map is bounded by the number of owners that self-invalidated
/// in the immediately preceding and current turns.
#[derive(Debug, Default)]
pub struct InvalidationGuard {
    streaks: HashMap<OwnerId, usize>,
}

impl InvalidationGuard {
    /// Advances the active owners by one display turn and returns every owner reaching `limit`.
    #[must_use]
    pub fn advance(&mut self, active: &[OwnerId], limit: usize) -> Vec<RunawayInvalidation> {
        let active = active.iter().copied().collect::<HashSet<_>>();
        self.streaks.retain(|owner, _| active.contains(owner));
        active
            .into_iter()
            .filter_map(|owner| {
                let streak = self.streaks.entry(owner).or_default();
                *streak = streak.saturating_add(1);
                (*streak >= limit).then_some(RunawayInvalidation {
                    owner,
                    streak: *streak,
                })
            })
            .collect()
    }

    /// Resets one owner after it recovers through an external invalidation.
    pub fn reset(&mut self, owner: OwnerId) {
        self.streaks.remove(&owner);
    }
}

#[cfg(test)]
mod tests {
    use anmixiu_reactive::OwnerRegistry;

    use super::InvalidationGuard;

    #[test]
    fn alternating_owners_never_accumulate_one_shared_streak() {
        let owners = OwnerRegistry::new();
        let first = owners.create_owner();
        let second = owners.create_owner();
        let mut guard = InvalidationGuard::default();

        for owner in [first, second].into_iter().cycle().take(16) {
            assert!(guard.advance(&[owner], 8).is_empty());
        }
    }

    #[test]
    fn one_owner_trips_at_its_own_consecutive_limit() {
        let owners = OwnerRegistry::new();
        let owner = owners.create_owner();
        let mut guard = InvalidationGuard::default();

        for _ in 0..7 {
            assert!(guard.advance(&[owner], 8).is_empty());
        }
        let runaway = guard.advance(&[owner], 8);
        assert_eq!(runaway.len(), 1);
        assert_eq!(runaway[0].owner, owner);
        assert_eq!(runaway[0].streak, 8);
    }

    #[test]
    fn reset_clears_only_the_recovered_owner() {
        let owners = OwnerRegistry::new();
        let recovered = owners.create_owner();
        let continuing = owners.create_owner();
        let mut guard = InvalidationGuard::default();

        for _ in 0..7 {
            assert!(guard.advance(&[recovered, continuing], 8).is_empty());
        }
        guard.reset(recovered);
        let runaway = guard.advance(&[recovered, continuing], 8);
        assert_eq!(runaway.len(), 1);
        assert_eq!(runaway[0].owner, continuing);
    }
}
