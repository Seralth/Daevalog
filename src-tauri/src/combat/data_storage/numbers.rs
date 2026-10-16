//! Player numbers for "Hide other players' names": the page shows another
//! player as their class and this number.

use std::collections::HashMap;

use super::DataStorage;

/// A number per player, handed out in the order the meter and Details first
/// show them and kept until the meter quits: through new fights, resets, mode
/// and zone changes. Kept here rather than in a window, so every window
/// numbers a player alike. Display only: never saved, never uploaded.
#[derive(Default, Clone)]
pub(super) struct PlayerNumbers {
    /// A name is the same player all session.
    by_name: HashMap<String, u32>,
    /// For a row the game has not named yet. Entity ids are handed out per
    /// zone load, so these go at each one (`forget_player_ids`).
    by_id: HashMap<i32, u32>,
    /// The name each number was given to. A number given to an unnamed id
    /// has none until the id is named.
    owner: HashMap<u32, String>,
    last: u32,
}

impl PlayerNumbers {
    fn number(&mut self, id: i32, name: &str) -> u32 {
        let name = name.trim();
        let named = !name.is_empty() && !name.chars().all(|c| c.is_ascii_digit());
        if !named {
            let n = self.by_id.get(&id).copied().unwrap_or_else(|| self.next());
            self.by_id.insert(id, n);
            return n;
        }
        let n = match self.by_name.get(name) {
            Some(&n) => n,
            None => {
                // An unnamed id that is named now keeps its number, unless
                // the number is someone else's: the id was named another
                // player before, and the backend has since named it anew.
                let n = match self.by_id.get(&id) {
                    Some(n) if !self.owner.contains_key(n) => *n,
                    _ => self.next(),
                };
                self.by_name.insert(name.to_string(), n);
                self.owner.insert(n, name.to_string());
                n
            }
        };
        self.by_id.insert(id, n);
        n
    }

    fn next(&mut self) -> u32 {
        self.last += 1;
        self.last
    }

    /// Ids may name someone else from now on; names keep their numbers.
    fn forget_ids(&mut self) {
        self.by_id.clear();
    }
}

impl DataStorage {
    /// The number of each `(id, name)`; players not seen before get the next
    /// numbers, in the order given.
    pub fn player_numbers<'a>(&self, players: impl IntoIterator<Item = (i32, &'a str)>) -> HashMap<i32, u32> {
        let mut numbers = self.player_numbers.lock();
        players.into_iter().map(|(id, name)| (id, numbers.number(id, name))).collect()
    }

    /// Ids name other players from now on (a zone load, a replay of another
    /// session); names keep their numbers.
    pub fn forget_player_ids(&self) {
        self.player_numbers.lock().forget_ids();
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
    fn numbers_last_the_session_through_resets_and_zone_changes() {
        let s = DataStorage::new();
        s.player_numbers([(100, "A"), (200, "B")]);
        s.clear_fight_totals();
        assert_eq!(s.player_numbers([(200, "B")])[&200], 2);
        s.flush();
        s.forget_names();
        assert_eq!(s.player_numbers([(300, "C"), (100, "A")]), HashMap::from([(300, 3), (100, 1)]));
        // A new zone hands out new ids: the same players, by name.
        s.note_map_load(1010);
        assert_eq!(s.player_numbers([(5000, "B"), (5001, "A")]), HashMap::from([(5000, 2), (5001, 1)]));
    }

    #[test]
    fn an_id_from_before_a_zone_load_does_not_name_a_new_player() {
        let s = DataStorage::new();
        assert_eq!(s.player_numbers([(300, "300")])[&300], 1);
        s.note_map_load(1010);
        // Id 300 in the new zone is someone the meter has not met.
        assert_eq!(s.player_numbers([(300, "300")])[&300], 2);
        assert_eq!(s.player_numbers([(300, "Named")])[&300], 2, "named, it keeps the number it had");
    }

    #[test]
    fn a_name_corrected_on_an_id_is_another_player() {
        let s = DataStorage::new();
        assert_eq!(s.player_numbers([(300, "Wrong")])[&300], 1);
        // The backend names id 300 anew; Wrong is still somewhere else.
        assert_eq!(s.player_numbers([(300, "Right"), (301, "Wrong")]), HashMap::from([(300, 2), (301, 1)]));
    }
}
