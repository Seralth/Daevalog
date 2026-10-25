// SPDX-License-Identifier: AGPL-3.0-only

//! Field boss notices: a countdown in the notice tray before Gartua and each
//! commander, the field bosses with the best gear, 30, 15 and 5 minutes
//! before, as for an Abyss Boss. The server sends a field boss's time only
//! while the world map is open (`01 91`), so the meter learns it when the
//! player opens the map and keeps it until the boss comes. A boss the map
//! shows up gets no countdown. Each boss is one notice, updated in place,
//! that goes 10 minutes after the boss comes; a new time from the map moves
//! it at once.

use std::collections::HashMap;

use daevalog::capture::decoder::WorldMapList;

use crate::i18n::text;
use crate::timed_notices::{expire, on_tray, reach, take_all, Move, Notice, Posted, Words, BOSS_LEADS, KEEP_AFTER};

/// The game's words for a field boss ("Field Boss" in its achievements for
/// these bosses): the name in the notice's title.
pub const FIELD_BOSS: &str = "notices.spawn.fieldBoss";

/// The keys this file looks up, for the strings check.
#[cfg(test)]
pub const KEYS: [&str; 1] = [FIELD_BOSS];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Tier {
    Gartua,
    Commander,
}

impl Tier {
    /// The notice's kind, which picks its icon and colour (NoticeIcon.qml).
    fn kind(self) -> &'static str {
        match self {
            Tier::Gartua => "gartua",
            Tier::Commander => "commander",
        }
    }
}

/// The field bosses with a countdown, by their row in the game's
/// WorldMapFieldNamed table (the world map's Field Boss list), with their
/// NPC: Eternal Gartua and its commanders on one side's map, Immortal Gartua
/// and its commanders on the other's.
const BOSSES: [(u32, u32, Tier); 8] = [
    (101_021, 2_101_074, Tier::Gartua),
    (101_022, 2_101_120, Tier::Commander),
    (101_023, 2_101_122, Tier::Commander),
    (101_024, 2_101_131, Tier::Commander),
    (111_021, 2_400_800, Tier::Gartua),
    (111_022, 2_400_853, Tier::Commander),
    (111_023, 2_400_854, Tier::Commander),
    (111_024, 2_400_855, Tier::Commander),
];

#[derive(Default)]
pub struct FieldBossNotices {
    /// By row: the boss's map and the unix second it comes next, from the
    /// newest map list that showed it not up.
    next: HashMap<u32, (u32, i64)>,
    /// (row, time, lead) posted, so each lead comes once.
    posted: Posted<u32>,
    /// Notices on the tray now, by row: the map and the time.
    shown: HashMap<u32, (u32, i64)>,
}

fn key(row: u32) -> String {
    format!("fieldboss:{row}")
}

fn npc_and_tier(row: u32) -> Option<(u32, Tier)> {
    BOSSES.iter().find(|(r, _, _)| *r == row).map(|&(_, npc, tier)| (npc, tier))
}

fn title(words: &Words, at: i64) -> String {
    words.title_named(&text(words.language, FIELD_BOSS), at)
}

impl FieldBossNotices {
    /// A map list: each boss's time. A boss on the tray whose time it moves
    /// is moved with it, or taken off when the new time is outside the
    /// stretch its notice shows in.
    pub fn learn(&mut self, list: &WorldMapList, words: &Words, now: i64) -> Vec<Move> {
        let mut moves = Vec::new();
        for boss in &list.bosses {
            if npc_and_tier(boss.row).is_none() {
                continue;
            }
            let at = (boss.time_ms / 1000) as i64;
            if boss.spawned {
                self.next.remove(&boss.row);
            } else {
                self.next.insert(boss.row, (list.map, at));
            }
            let Some(shown) = self.shown.get_mut(&boss.row) else { continue };
            if shown.1 == at {
                continue;
            }
            if on_tray(at, now, reach(&BOSS_LEADS)) {
                *shown = (list.map, at);
                moves.push(Move::Retime { key: key(boss.row), title: title(words, at), until: at });
            } else {
                self.shown.remove(&boss.row);
                moves.push(Move::Remove(key(boss.row)));
            }
        }
        moves
    }

    /// The notices due at `now`, one per boss.
    pub fn due(&mut self, words: &Words, now: i64) -> Vec<Notice> {
        let mut out = Vec::new();
        for (&row, &(map, at)) in &self.next {
            let Some((npc, tier)) = npc_and_tier(row) else { continue };
            if self.posted.due_lead(row, at, now, &BOSS_LEADS).is_none() {
                continue;
            }
            self.shown.insert(row, (map, at));
            out.push(Notice { key: key(row), kind: tier.kind(), title: title(words, at), text: words.boss_line(npc, map), until: at });
        }
        self.next.retain(|_, &mut (_, at)| now < at + KEEP_AFTER);
        self.posted.forget_old(now);
        out.sort_by(|a, b| (a.until, &a.key).cmp(&(b.until, &b.key)));
        out
    }

    /// The keys of notices whose boss came long enough ago; they leave the list.
    pub fn expired(&mut self, now: i64) -> Vec<String> {
        expire(&mut self.shown, now, |&(_, at)| at).into_iter().map(key).collect()
    }

    /// Every notice this has on the tray, for when the setting goes off; the
    /// times are kept.
    pub fn take_all(&mut self) -> Vec<String> {
        take_all(&mut self.posted, &mut self.shown).into_iter().map(key).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timed_notices::test_words::{names, utc, words};
    use crate::timed_notices::LATE;
    use daevalog::capture::decoder::FieldBossTime;

    /// A day at midnight UTC.
    const DAY0: i64 = 1_792_368_000;

    fn list(map: u32, bosses: &[(u32, bool, i64)]) -> WorldMapList {
        WorldMapList {
            map,
            bosses: bosses.iter().map(|&(row, spawned, at)| FieldBossTime { row, spawned, time_ms: at as u64 * 1000 + 250 }).collect(),
        }
    }

    #[test]
    fn gartua_and_the_commanders_count_down_30_15_and_5_minutes_before_each_in_its_own_notice() {
        let n = names();
        let w = words(&n, &utc);
        let mut notices = FieldBossNotices::default();
        let (gartua, dartan) = (DAY0 + 5 * 3600, DAY0 + 5 * 3600 + 4 * 60);
        // 101001 is a lesser boss: no countdown.
        assert!(notices.learn(&list(1010, &[(101_021, false, gartua), (101_022, false, dartan), (101_001, false, gartua)]), &w, DAY0).is_empty());
        for lead in BOSS_LEADS {
            let got = notices.due(&w, gartua - lead * 60);
            assert_eq!(got.len(), 1, "lead {lead}");
            assert_eq!(got[0].key, "fieldboss:101021");
            assert_eq!(got[0].kind, "gartua");
            assert_eq!(got[0].title, "Field Boss at 05:00");
            assert_eq!(got[0].text, "Green Golem (Map One)");
            assert_eq!(got[0].until, gartua);
            assert!(notices.due(&w, gartua - lead * 60 + 20).is_empty(), "once per lead");
            let got = notices.due(&w, dartan - lead * 60);
            assert_eq!(got.len(), 1);
            assert_eq!((got[0].key.as_str(), got[0].kind), ("fieldboss:101022", "commander"));
            assert_eq!(got[0].text, "Quiet One (Map One)");
        }
    }

    #[test]
    fn a_boss_up_on_the_map_gets_no_countdown() {
        let n = names();
        let w = words(&n, &utc);
        let mut notices = FieldBossNotices::default();
        let at = DAY0 + 3600;
        notices.learn(&list(1010, &[(101_021, false, at)]), &w, DAY0);
        // Up early by an hour: the map's time is when it came.
        notices.learn(&list(1010, &[(101_021, true, at - 3600)]), &w, DAY0);
        assert!(notices.due(&w, at - 30 * 60).is_empty());
        assert!(notices.due(&w, at - 5 * 60).is_empty());
    }

    #[test]
    fn a_dismissed_notice_comes_back_at_the_next_lead_and_all_go_when_switched_off() {
        let n = names();
        let w = words(&n, &utc);
        let mut notices = FieldBossNotices::default();
        let at = DAY0 + 3600;
        notices.learn(&list(1110, &[(111_021, false, at)]), &w, DAY0);
        assert_eq!(notices.due(&w, at - 30 * 60)[0].text, "Other Golem (Map Two)");
        // The tray takes it off; the next lead posts it again under its key.
        assert_eq!(notices.due(&w, at - 15 * 60)[0].key, "fieldboss:111021");
        assert_eq!(notices.take_all(), ["fieldboss:111021"]);
        assert!(notices.expired(at + KEEP_AFTER).is_empty());
    }

    #[test]
    fn a_late_map_posts_the_lead_within_three_minutes_and_not_after() {
        let n = names();
        let w = words(&n, &utc);
        let at = DAY0 + 3600;
        let mut notices = FieldBossNotices::default();
        // The map opened 20 minutes before: only the 5 minute notice comes.
        notices.learn(&list(1010, &[(101_023, false, at)]), &w, at - 20 * 60);
        assert!(notices.due(&w, at - 20 * 60).is_empty());
        assert_eq!(notices.due(&w, at - 5 * 60 + LATE - 1).len(), 1);
        let mut notices = FieldBossNotices::default();
        notices.learn(&list(1010, &[(101_023, false, at)]), &w, at - 20 * 60);
        assert!(notices.due(&w, at - 5 * 60 + LATE).is_empty());
    }

    #[test]
    fn a_notice_goes_ten_minutes_after_its_boss_comes() {
        let n = names();
        let w = words(&n, &utc);
        let mut notices = FieldBossNotices::default();
        let at = DAY0 + 3600;
        notices.learn(&list(1010, &[(101_024, false, at)]), &w, DAY0);
        notices.due(&w, at - 5 * 60);
        assert!(notices.expired(at + KEEP_AFTER - 1).is_empty());
        assert_eq!(notices.expired(at + KEEP_AFTER), ["fieldboss:101024"]);
        assert!(notices.due(&w, at + KEEP_AFTER).is_empty());
    }

    #[test]
    fn a_new_time_from_the_map_moves_the_notice_at_once_or_takes_it_off() {
        let n = names();
        let w = words(&n, &utc);
        let mut notices = FieldBossNotices::default();
        let at = DAY0 + 3600;
        notices.learn(&list(1010, &[(101_021, false, at)]), &w, DAY0);
        notices.due(&w, at - 15 * 60);
        // The same time again moves nothing.
        assert!(notices.learn(&list(1010, &[(101_021, false, at)]), &w, at - 14 * 60).is_empty());
        // Up two minutes early: the notice counts to when it came.
        let moved = notices.learn(&list(1010, &[(101_021, true, at - 120)]), &w, at - 60);
        assert_eq!(moved, [Move::Retime { key: "fieldboss:101021".into(), title: "Field Boss at 00:58".into(), until: at - 120 }]);
        assert_eq!(notices.expired(at - 120 + KEEP_AFTER), ["fieldboss:101021"], "goes after the new time");
        // Killed while its notice shows: the next time is hours away, so it goes now.
        notices.learn(&list(1010, &[(101_022, false, at)]), &w, DAY0);
        notices.due(&w, at - 5 * 60);
        let moved = notices.learn(&list(1010, &[(101_022, false, at + 6 * 3600)]), &w, at + 60);
        assert_eq!(moved, [Move::Remove("fieldboss:101022".into())]);
        assert!(notices.expired(at + KEEP_AFTER).is_empty());
        assert_eq!(notices.due(&w, at + 6 * 3600 - 30 * 60).len(), 1, "and comes back for the new time");
    }
}
