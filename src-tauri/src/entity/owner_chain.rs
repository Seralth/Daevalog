//! Who a summon's damage counts for: the end of its chain of owners.

use std::collections::HashMap;

/// The most owner links followed for one actor.
const MAX_HOPS: usize = 16;

/// The owner an actor's hits count for. `summon_data` maps a summon's id to
/// its owner's id. Follows owners until an id has no owner, an id comes up a
/// second time, or `MAX_HOPS` links were followed; the id reached then is the
/// answer. Ids of 0 and below are no entity and come back as they are.
pub fn resolve(actor_id: i32, summon_data: &HashMap<i32, i32>) -> i32 {
    if actor_id <= 0 {
        return actor_id;
    }
    let mut path = Vec::with_capacity(4);
    let mut here = actor_id;
    while path.len() < MAX_HOPS {
        let next = match summon_data.get(&here) {
            Some(&owner) if owner > 0 => owner,
            _ => break,
        };
        path.push(here);
        if path.contains(&next) {
            return next;
        }
        here = next;
    }
    here
}

#[cfg(test)]
mod tests {
    use super::resolve;
    use std::collections::HashMap;

    fn links(pairs: &[(i32, i32)]) -> HashMap<i32, i32> {
        pairs.iter().copied().collect()
    }

    #[test]
    fn no_entity_ids_are_never_looked_up() {
        assert_eq!(resolve(0, &links(&[(0, 5)])), 0);
        assert_eq!(resolve(-3, &links(&[(-3, 5)])), -3);
    }

    #[test]
    fn an_actor_with_no_owner_is_its_own() {
        assert_eq!(resolve(7, &HashMap::new()), 7);
        assert_eq!(resolve(5, &links(&[(5, 0)])), 5);
        assert_eq!(resolve(1, &links(&[(1, 2), (2, -1)])), 2);
    }

    #[test]
    fn owners_are_followed_to_the_end() {
        assert_eq!(resolve(52467, &links(&[(52467, 9063)])), 9063);
        let chain = links(&[(1, 2), (2, 3)]);
        assert_eq!([1, 2, 3].map(|a| resolve(a, &chain)), [3, 3, 3]);
    }

    #[test]
    fn a_loop_ends_at_the_first_repeat() {
        let pair = links(&[(1, 2), (2, 1)]);
        assert_eq!(resolve(1, &pair), 1);
        assert_eq!(resolve(2, &pair), 2);
        assert_eq!(resolve(4, &links(&[(4, 4)])), 4);
        assert_eq!(resolve(1, &links(&[(1, 2), (2, 3), (3, 2)])), 2);
    }

    #[test]
    fn at_most_sixteen_links_are_followed() {
        let chain: HashMap<i32, i32> = (1..20).map(|n| (n, n + 1)).collect();
        assert_eq!(resolve(1, &chain), 17);
        assert_eq!(resolve(4, &chain), 20);
        assert_eq!(resolve(19, &chain), 20);
    }
}
