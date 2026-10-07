//! Player numbers for "Hide other players' names": the page shows another
//! player as their class and this number.

use std::collections::HashMap;

use super::DataStorage;

/// A number per player, handed out in the order the meter and Details first
/// show them and kept until combat is cleared. Kept here rather than in a
/// window, so every window numbers a player alike. Display only: never saved,
/// never uploaded.
#[derive(Default)]
pub(super) struct PlayerNumbers {
    by_name: HashMap<String, u32>,
    by_id: HashMap<i32, u32>,
    last: u32,
}

impl PlayerNumbers {
    fn number(&mut self, id: i32, name: &str) -> u32 {
        // A party member's row moves from a placeholder id to their real one,
        // and an unnamed id is named later: the number stays either way.
        let name = name.trim();
        let named = !name.is_empty() && !name.chars().all(|c| c.is_ascii_digit());
        let known = if named { self.by_name.get(name).copied() } else { None };
        let n = known.or_else(|| self.by_id.get(&id).copied()).unwrap_or_else(|| {
            self.last += 1;
            self.last
        });
        self.by_id.insert(id, n);
        if named {
            self.by_name.insert(name.to_string(), n);
        }
        n
    }
}

impl DataStorage {
    /// The number of each `(id, name)`; players not seen before get the next
    /// numbers, in the order given.
    pub fn player_numbers<'a>(&self, players: impl IntoIterator<Item = (i32, &'a str)>) -> HashMap<i32, u32> {
        let mut numbers = self.player_numbers.lock();
        players.into_iter().map(|(id, name)| (id, numbers.number(id, name))).collect()
    }

    pub(super) fn clear_player_numbers(&self) {
        *self.player_numbers.lock() = PlayerNumbers::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_player_keeps_one_number_through_a_new_id_or_a_late_name() {
        let s = DataStorage::new();
        let first = s.player_numbers([(90_000_001, "Placeholder"), (300, "300")]);
        assert_eq!((first[&90_000_001], first[&300]), (1, 2));
        // The placeholder's real id arrives, and id 300 is named.
        let later = s.player_numbers([(200, "Placeholder"), (300, "Named"), (400, "New")]);
        assert_eq!((later[&200], later[&300], later[&400]), (1, 2, 3));
        assert_eq!(s.player_numbers([(301, "Named")])[&301], 2);
    }

    #[test]
    fn clearing_combat_starts_the_numbers_again() {
        let s = DataStorage::new();
        s.player_numbers([(100, "A"), (200, "B")]);
        s.flush_combat_only();
        assert_eq!(s.player_numbers([(200, "B")])[&200], 1);
        s.flush();
        assert_eq!(s.player_numbers([(300, "C")])[&300], 1);
    }
}
