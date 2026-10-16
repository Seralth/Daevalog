// SPDX-License-Identifier: AGPL-3.0-only

//! One reading of the meter, laid out for the window: the rows the backend's
//! meter view made of it (`daevalog::combat::meter_view`), in its order,
//! plus what the header shows and the Heal and Tank tabs. Every number and
//! every row rule is the backend's; this only labels and formats them.

use std::collections::HashMap;

use daevalog::combat::meter_view::{FightClock, Party, PartyRow, Shown, ViewRow};
use daevalog::entity::dps_data::DpsData;
use daevalog::entity::job_class::JobClass;

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub id: i32,
    pub name: String,
    /// No name yet: `name` is the id.
    pub identifying: bool,
    /// Theme key for the colour and icon ("gladiator"); empty when unknown.
    pub class: &'static str,
    pub rank: usize,
    pub damage: i64,
    pub dps: i64,
    /// Share of the damage of the rows shown, in percent.
    pub share: f64,
    /// Bar length, 0 to 1.
    pub fill: f64,
    pub you: bool,
    /// The one row for summons and effects that no player owns.
    pub unattributed: bool,
    /// Another player's number, shown with their class when other players'
    /// names are hidden; 0 for you.
    pub number: u32,
    /// DPS from the player's first hit to their last, and over the last 10,
    /// 30 and 60 seconds.
    pub active_dps: f64,
    pub last10_dps: f64,
    pub last30_dps: f64,
    pub last60_dps: f64,
    pub hits: i64,
    pub crit_hits: i64,
    pub max_hit: i64,
    /// From the party roster; 0 for anyone outside the party.
    pub combat_power: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    /// Time of the reading, unix ms: capture time in a replay.
    pub at_ms: i64,
    pub target_id: i32,
    pub target_name: String,
    /// The target's NPC code; 0 with none.
    pub target_code: i32,
    /// The backend's mode id ("bossTargets").
    pub mode: String,
    pub battle_ms: i64,
    /// The fight is open: its time moved in the last 30 s (the meter view's
    /// clock fighting or in grace), its target is not dead, and the server
    /// has not taken it out of combat.
    pub fight_open: bool,
    /// The fight time at which the target enrages, from the server's
    /// deadline; 0 with none (`crate::enrage` then counts from its table).
    pub enrage_ms: i64,
    /// Boss health left, 0 to 1; negative when there is no health bar.
    pub boss_health: f64,
    /// The health left and the most, for the bar's text; `None` with no bar.
    pub boss_hp: Option<(i64, i64)>,
    /// The target's HP as its own `00 8D` records last gave it, for the time
    /// to kill; -1 with none.
    pub boss_live_hp: i64,
    /// The time to kill in whole seconds (`HpTrail`), set by the reading
    /// loop; `None` before there is one.
    pub boss_eta: Option<i64>,
    /// In display order.
    pub rows: Vec<Row>,
    /// The footer's group DPS; `None` with fewer than two rows.
    pub group_dps: Option<i64>,
    /// The group's damage, beside the group DPS; `None` with fewer than two
    /// rows.
    pub group_damage: Option<i64>,
    /// The Heal and Tank tabs (`crate::party`); `None` before the first
    /// fight.
    pub party: Option<Party>,
    /// Combat power by character name, for the Heal and Tank rows: the
    /// roster joins on the name too.
    pub combat_powers: HashMap<String, i64>,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            at_ms: 0,
            target_id: 0,
            target_name: String::new(),
            target_code: 0,
            mode: String::new(),
            battle_ms: 0,
            fight_open: false,
            enrage_ms: 0,
            boss_health: -1.0,
            boss_hp: None,
            boss_live_hp: -1,
            boss_eta: None,
            rows: Vec::new(),
            group_dps: None,
            group_damage: None,
            party: None,
            combat_powers: HashMap::new(),
        }
    }
}

/// The backend names classes in Korean (`JobClass::class_name`); the theme
/// by these keys.
const CLASS_KEYS: [(JobClass, &str); 9] = [
    (JobClass::Gladiator, "gladiator"),
    (JobClass::Templar, "templar"),
    (JobClass::Ranger, "ranger"),
    (JobClass::Assassin, "assassin"),
    (JobClass::Sorcerer, "sorcerer"),
    (JobClass::Cleric, "cleric"),
    (JobClass::Elementalist, "spiritmaster"),
    (JobClass::Chanter, "chanter"),
    (JobClass::Fighter, "fighter"),
];

pub fn class_key(job: &str) -> &'static str {
    CLASS_KEYS
        .iter()
        .find(|(class, _)| class.class_name() == job)
        .map_or("", |(_, key)| key)
}

impl Row {
    fn new(row: &ViewRow) -> Self {
        Self {
            id: row.id,
            name: row.name.clone(),
            identifying: row.identifying,
            class: class_key(&row.job),
            rank: row.rank,
            damage: row.damage,
            dps: row.dps,
            share: row.share,
            fill: row.fill,
            you: row.you,
            unattributed: row.unattributed,
            number: row.number,
            active_dps: row.active_dps,
            last10_dps: row.last10_dps,
            last30_dps: row.last30_dps,
            last60_dps: row.last60_dps,
            hits: row.hits,
            crit_hits: row.crit_hits,
            max_hit: row.max_hit,
            combat_power: row.combat_power,
        }
    }

    /// The text of one of the DPS tab's columns (settings_store's
    /// `DPS_COLUMNS`); every rate follows Round DPS, as in 1.x.
    pub fn cell(&self, key: &str, round: bool) -> String {
        let per_second = |v: f64| if round { rate(v) } else { whole(v) };
        match key {
            "total" => damage(self.damage as f64),
            "encDps" => per_second(self.dps as f64),
            "dps" => per_second(self.active_dps),
            "pct" => share(self.share),
            // Rounded half up, as 1.x's Math.round.
            "crit" if self.hits > 0 => format!("{}%", (self.crit_hits as f64 / self.hits as f64 * 100.0).round()),
            "last10" => per_second(self.last10_dps),
            "last30" => per_second(self.last30_dps),
            "last60" => per_second(self.last60_dps),
            "maxHit" if self.hits > 0 => damage(self.max_hit as f64),
            "hits" if self.hits > 0 => whole(self.hits as f64),
            // No hits from the game (world bosses send only other players'
            // ticks): 0 would read as missing every time.
            "crit" | "maxHit" | "hits" => "-".into(),
            _ => String::new(),
        }
    }

    /// Damage with no hit behind it: the game sent only damage over time
    /// ticks for this player. Its "-" cells say so on hover.
    pub fn ticks_only(&self) -> bool {
        self.hits == 0 && self.damage > 0
    }
}

impl Snapshot {
    /// A reading and the rows the meter view made of it.
    pub fn new(reading: &DpsData, shown: &Shown, at_ms: i64) -> Self {
        Self {
            at_ms,
            target_id: reading.target_id,
            target_name: reading.target_name.clone(),
            target_code: reading.target_mob_code,
            mode: reading.target_mode.clone(),
            battle_ms: reading.battle_time,
            fight_open: matches!(shown.clock, FightClock::Fighting | FightClock::Grace)
                && !(reading.target_max_hp > 0 && reading.target_current_hp == 0)
                && !reading.target_out_of_combat,
            enrage_ms: reading.target_enrage_ms,
            boss_health: boss_health(reading),
            boss_hp: boss_hp(reading),
            boss_live_hp: if reading.target_max_hp > 0 { reading.target_current_hp } else { -1 },
            boss_eta: None,
            rows: shown.rows.iter().map(Row::new).collect(),
            group_dps: shown.total.map(|t| t.dps),
            group_damage: shown.total.map(|t| t.damage),
            party: None,
            // The rows kept on screen after a fight too: theirs as the
            // roster has it now (`meter_view`), none for a player who left.
            combat_powers: reading
                .map
                .values()
                .map(|p| (p.nickname.clone(), p.combat_power))
                .chain(shown.rows.iter().map(|r| (r.name.clone(), r.combat_power)))
                .filter(|(_, cp)| *cp > 0)
                .collect(),
        }
    }
}

/// The text of one of the Heal or Tank tab's columns (settings_store's
/// `HEAL_COLUMNS`, `TANK_COLUMNS`); rates follow Round DPS, as on the DPS
/// tab. A number with no record behind it shows "-", a true 0 stays 0;
/// damage over time ticks received are no attack, so damage counts too.
pub fn party_cell(row: &PartyRow, key: &str, round: bool) -> String {
    let per_second = |v: Option<f64>| match v {
        Some(v) if round => rate(v),
        Some(v) => whole(v),
        None => "-".into(),
    };
    let none = match key {
        "healed" | "hps" | "dtps" | "pct" => row.records == 0 && row.amount == 0,
        "taken" | "block" | "parry" => row.attacks == 0 && row.taken == 0,
        _ => false,
    };
    if none {
        return "-".into();
    }
    match key {
        "healed" => damage(row.amount as f64),
        "taken" => damage(row.taken as f64),
        "hps" | "dtps" => per_second(row.per_second),
        "pct" => share(row.share),
        "block" => whole(row.shield_block as f64),
        "parry" => whole(row.parry as f64),
        _ => String::new(),
    }
}

/// The live HP reading when there is one, else max HP less the damage seen.
fn boss_health(meter: &DpsData) -> f64 {
    let max = meter.target_max_hp;
    if max <= 0 {
        return -1.0;
    }
    let left = if meter.target_current_hp >= 0 {
        meter.target_current_hp.min(max)
    } else {
        (max - meter.target_total_damage.max(0)).max(0)
    };
    left as f64 / max as f64
}

/// The health left and the most, as `boss_health` reads them.
fn boss_hp(meter: &DpsData) -> Option<(i64, i64)> {
    let max = meter.target_max_hp;
    (max > 0).then(|| {
        let left = if meter.target_current_hp >= 0 {
            meter.target_current_hp.min(max)
        } else {
            (max - meter.target_total_damage.max(0)).max(0)
        };
        (left, max)
    })
}

/// The time to kill at the pace of the pull so far, from the target's own HP
/// records, so damage the meter does not count (other players', unknown
/// damage over time) counts too: whole seconds, or none before `ETA_MIN_MS`
/// of readings or while the HP is not below where the pull began. The pace of
/// the whole pull rather than of the last 20 s: on long boss pulls it jumps
/// far less often and is closer to the real kill time.
pub const ETA_MIN_MS: i64 = 5_000;

/// A rise in HP of more than this share of the pull's first HP (a reset, a
/// wipe, a new pull on the same id) starts the pull again; smaller heals do
/// not.
const ETA_NEW_PULL_RISE: f64 = 0.02;

/// The pull's first HP reading and its latest (time, HP), kept by the
/// reading loop, which sees every reading (a fast replay sends the window
/// only some).
#[derive(Debug, Default)]
pub struct HpTrail {
    target: i32,
    first: Option<(i64, i64)>,
    last: Option<(i64, i64)>,
}

impl HpTrail {
    /// The time to kill after the reading at `at`: `hp` is the target's HP
    /// from its records, -1 with none.
    pub fn update(&mut self, at: i64, target: i32, hp: i64) -> Option<i64> {
        if self.target != target || hp < 0 || self.last.is_some_and(|(t, _)| at < t) {
            (self.first, self.last) = (None, None);
            self.target = target;
        }
        if hp >= 0 {
            let new_pull = match (self.first, self.last) {
                (Some((_, start)), Some((_, before))) => (hp - before) as f64 > ETA_NEW_PULL_RISE * start as f64,
                _ => true,
            };
            if new_pull {
                self.first = Some((at, hp));
            }
            self.last = Some((at, hp));
        }
        time_to_kill(self.first, self.last)
    }
}

/// The HP left over the HP lost per second from the pull's first reading to
/// its last.
pub fn time_to_kill(first: Option<(i64, i64)>, last: Option<(i64, i64)>) -> Option<i64> {
    let (Some((t0, hp0)), Some((t1, hp1))) = (first, last) else {
        return None;
    };
    if t1 - t0 < ETA_MIN_MS || hp1 >= hp0 {
        return None;
    }
    let per_ms = (hp0 - hp1) as f64 / (t1 - t0) as f64;
    Some((hp1 as f64 / per_ms / 1_000.0).round() as i64)
}

/// Whole seconds as m:ss, for the time to kill and the enrage countdown.
pub fn clock(secs: i64) -> String {
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The fight time the header shows. The backend's moves only with a hit, so
/// between hits the clock stood still and looked the same as a meter that
/// stopped capturing. While the fight is open it runs on from the last
/// move, every second; a hit puts it back on the backend's time.
#[derive(Debug, Default)]
pub struct FightTicker {
    battle_ms: i64,
    moved_at: i64,
}

impl FightTicker {
    pub fn update(&mut self, at: i64, battle_ms: i64, open: bool) -> i64 {
        if battle_ms != self.battle_ms {
            self.battle_ms = battle_ms;
            self.moved_at = at;
        }
        if open { battle_ms + (at - self.moved_at).max(0) } else { battle_ms }
    }
}

/// Fight time as mm:ss.
pub fn fight_time(ms: i64) -> String {
    let secs = ms.max(0) / 1000;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

/// Damage: millions from 100K up ("0.41M", "2.61M"), else thousands.
pub fn damage(v: f64) -> String {
    if v >= 100_000.0 {
        format!("{:.2}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{v:.0}")
    }
}

/// Per second: "19.5K", "980", "1.25M".
pub fn rate(v: f64) -> String {
    if v >= 1_000_000.0 {
        format!("{:.2}M", v / 1_000_000.0)
    } else if v >= 1_000.0 {
        format!("{:.1}K", v / 1_000.0)
    } else {
        format!("{v:.0}")
    }
}

/// Combat power beside a name, in thousands as players quote it: 889,100
/// is "889K", 889,545 "890K"; below 500 the number itself; none for 0.
pub fn combat_power(v: i64) -> String {
    if v <= 0 {
        return String::new();
    }
    let thousands = (v as f64 / 1_000.0).round();
    if thousands > 0.0 { format!("{}K", whole(thousands)) } else { whole(v as f64) }
}

/// A share: whole percent from 10% up, one decimal below, as 1.x writes
/// every share, so 4.7% does not read as 5% beside the low-share colours.
pub fn share(v: f64) -> String {
    let tenths = (v * 10.0).round() / 10.0;
    if tenths < 10.0 { format!("{tenths:.1}%") } else { format!("{v:.0}%") }
}

/// The whole number with thousands separators, "19,512": a rate when Round
/// DPS is off, as 1.x writes it.
pub fn whole(v: f64) -> String {
    let n = v.round() as i64;
    let digits = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    if n < 0 { format!("-{out}") } else { out }
}

/// The footer's group DPS.
pub fn group(dps: i64, round: bool) -> String {
    if round { rate(dps as f64) } else { whole(dps as f64) }
}

#[cfg(test)]
mod tests {
    use super::*;
    use daevalog::combat::data_storage::UNATTRIBUTED_ID;
    use daevalog::combat::meter_view::{MeterView, ViewOptions};
    use daevalog::entity::personal_data::PersonalData;

    use crate::settings_store::DPS_COLUMNS;

    fn player(name: &str, job: JobClass, amount: f64, dps: f64) -> PersonalData {
        let mut p = PersonalData::with_job(name.into(), job.class_name().into());
        p.amount = amount;
        p.dps = dps;
        p
    }

    #[test]
    fn rows_come_as_the_backend_orders_them_with_your_row_marked() {
        let mut meter = DpsData::new();
        meter.map.insert(11, player("a", JobClass::Templar, 980_000.0, 7_300.0));
        meter.map.insert(12, player("b", JobClass::Sorcerer, 2_610_000.0, 19_500.0));
        meter.map.insert(13, player("c", JobClass::Elementalist, 1_940_000.0, 14_500.0));
        meter.map.insert(UNATTRIBUTED_ID, PersonalData::with_job("80000000".into(), "Unknown".into()));
        meter.map.insert(14, PersonalData::with_job("14".into(), "Unknown".into()));
        meter.local_player_id = Some(13);
        let shown = MeterView::new().update(&meter, "", 5, &ViewOptions::default()).clone();
        let snap = Snapshot::new(&meter, &shown, 5);
        let order: Vec<i32> = snap.rows.iter().map(|r| r.id).collect();
        assert_eq!(order, vec![12, 13, 11, 14, UNATTRIBUTED_ID]);
        assert!(snap.rows[1].you && !snap.rows[0].you);
        assert_eq!(snap.rows[1].class, "spiritmaster");
        assert_eq!(snap.rows[2].rank, 3);
        assert!(snap.rows[3].identifying && snap.rows[3].class.is_empty());
        assert!(snap.rows[4].unattributed);
        assert_eq!(snap.rows[1].fill, 14_500.0 / 19_500.0);
        assert_eq!(snap.group_dps, Some(41_300));
        assert_eq!(snap.group_damage, Some(5_530_000), "the group's damage beside it");
    }

    #[test]
    fn the_clock_runs_between_hits_while_the_fight_is_open() {
        let mut meter = DpsData::new();
        meter.map.insert(11, player("a", JobClass::Templar, 980_000.0, 7_300.0));
        meter.battle_time = 5_000;
        let mut view = MeterView::new();
        let mut ticker = FightTicker::default();
        let mut clock = |meter: &DpsData, at: i64| {
            let shown = view.update(meter, "", at, &ViewOptions::default()).clone();
            let snap = Snapshot::new(meter, &shown, at);
            fight_time(ticker.update(at, snap.battle_ms, snap.fight_open))
        };
        assert_eq!(clock(&meter, 100_000), "00:05");
        assert_eq!(clock(&meter, 102_400), "00:07", "no hit for 2.4 s: it runs on");
        assert_eq!(clock(&meter, 129_900), "00:34", "in grace too");
        assert_eq!(clock(&meter, 130_000), "00:05", "30 s without a hit ends the fight: its own time");
        meter.battle_time = 8_000;
        assert_eq!(clock(&meter, 131_000), "00:08", "a hit: the backend's time again");
        assert_eq!(clock(&meter, 133_000), "00:10");
        meter.battle_time = 9_000;
        assert_eq!(clock(&meter, 134_000), "00:09");
        meter.target_out_of_combat = true;
        assert_eq!(clock(&meter, 136_000), "00:09", "the server ended it: at once too");
        meter.target_out_of_combat = false;
        meter.target_max_hp = 1_000;
        meter.target_current_hp = 0;
        assert_eq!(clock(&meter, 137_000), "00:09", "a dead target ends it at once");
    }

    #[test]
    fn boss_health_prefers_the_live_reading() {
        let mut meter = DpsData::new();
        assert!(boss_health(&meter) < 0.0, "no bar without max HP");
        meter.target_max_hp = 1_000;
        meter.target_total_damage = 400;
        assert_eq!(boss_health(&meter), 0.6);
        meter.target_current_hp = 250;
        assert_eq!(boss_health(&meter), 0.25);
        assert_eq!(boss_hp(&meter), Some((250, 1_000)), "the bar's text");
        meter.target_current_hp = -1;
        meter.target_total_damage = 5_000;
        assert_eq!(boss_health(&meter), 0.0);
        assert_eq!(boss_hp(&meter), Some((0, 1_000)));
        meter.target_max_hp = 0;
        assert_eq!(boss_hp(&meter), None);
    }

    #[test]
    fn the_time_to_kill_follows_the_pace_of_the_pull() {
        let mut trail = HpTrail::default();
        // 10,000 HP a second from 1,000,000.
        let hp = |s: i64| 1_000_000 - 10_000 * s;
        for s in 0..5 {
            assert_eq!(trail.update(s * 1_000, 7, hp(s)), None, "under 5 s of readings");
        }
        assert_eq!(trail.update(5_000, 7, hp(5)), Some(95), "950,000 left at 10,000 a second");
        for s in 6..=30 {
            trail.update(s * 1_000, 7, hp(s));
        }
        // The pace doubles: the whole pull's pace follows, slower.
        for s in 31..=50 {
            trail.update(s * 1_000, 7, hp(30) - 20_000 * (s - 30));
        }
        let left = hp(30) - 20_000 * 21;
        assert_eq!(trail.update(51_000, 7, left), Some(20), "280,000 left at 720,000 over 51 s");
        // A phase the boss cannot be hurt in: the time stays up and grows.
        for s in 52..=72 {
            trail.update(s * 1_000, 7, 260_000);
        }
        assert_eq!(trail.update(73_000, 7, 260_000), Some(26), "740,000 over 73 s");
        // A small heal (under 2% of where the pull began) keeps the pull.
        assert_eq!(trail.update(74_000, 7, 270_000), Some(27));
        // A reset: a new pull, 5 s of readings first.
        assert_eq!(trail.update(75_000, 7, 1_000_000), None);
        assert_eq!(trail.update(80_000, 7, 900_000), Some(45), "100,000 over 5 s");
        // Another target, or no HP records: start again.
        assert_eq!(trail.update(81_000, 8, 500_000), None);
        assert_eq!(trail.update(82_000, 8, -1), None);
        assert_eq!(trail.update(87_000, 8, 400_000), None, "the 5 s start again");
        assert_eq!(trail.update(87_000, 8, 0), None, "still under 5 s");
        assert_eq!(trail.update(92_000, 8, 0), Some(0));
        assert_eq!((clock(95), clock(0), clock(600)), ("1:35".into(), "0:00".into(), "10:00".into()), "as m:ss");
    }

    #[test]
    fn numbers_read_as_in_the_mockup() {
        assert_eq!(damage(2_610_000.0), "2.61M");
        assert_eq!(damage(410_000.0), "0.41M");
        assert_eq!(damage(41_450.0), "41.5K");
        assert_eq!(damage(812.0), "812");
        assert_eq!(rate(19_500.0), "19.5K");
        assert_eq!(rate(980.4), "980");
        assert_eq!(share(43.6), "44%");
        assert_eq!(share(4.66), "4.7%", "one decimal under 10%");
        assert_eq!(share(9.96), "10%", "not 10.0%");
        assert_eq!(share(0.0), "0.0%");
        assert_eq!(fight_time(134_999), "02:14");
        assert_eq!(fight_time(-5), "00:00");
    }

    #[test]
    fn combat_power_reads_in_thousands_beside_the_name() {
        assert_eq!(combat_power(889_100), "889K");
        assert_eq!(combat_power(889_545), "890K");
        assert_eq!(combat_power(1_234_567), "1,235K");
        assert_eq!(combat_power(500), "1K");
        assert_eq!(combat_power(499), "499", "not 0K");
        assert_eq!(combat_power(0), "", "outside the party");

        let mut meter = DpsData::new();
        let mut a = player("a", JobClass::Templar, 980_000.0, 7_300.0);
        a.combat_power = 44_315;
        meter.map.insert(11, a);
        meter.map.insert(12, player("b", JobClass::Sorcerer, 20_000.0, 300.0));
        let shown = MeterView::new().update(&meter, "", 5, &ViewOptions::default()).clone();
        let snap = Snapshot::new(&meter, &shown, 5);
        assert_eq!((snap.rows[0].combat_power, snap.rows[1].combat_power), (44_315, 0));
        assert_eq!(snap.combat_powers, HashMap::from([("a".to_string(), 44_315)]), "by name, for the Heal and Tank rows");
    }

    #[test]
    fn round_dps_off_writes_the_whole_number() {
        assert_eq!(whole(19_512.4), "19,512");
        assert_eq!(whole(1_012_326.0), "1,012,326");
        assert_eq!(whole(980.0), "980");
        assert_eq!(whole(0.0), "0");
        assert_eq!(group(41_300, true), "41.3K");
        assert_eq!(group(41_300, false), "41,300");
    }

    #[test]
    fn each_heal_and_tank_column_has_its_number() {
        use crate::settings_store::{HEAL_COLUMNS, TANK_COLUMNS};
        let row = PartyRow {
            id: 11,
            name: "a".into(),
            identifying: false,
            unattributed: false,
            job: String::new(),
            you: false,
            number: 0,
            rank: 1,
            amount: 468_300,
            per_second: Some(6_504.2),
            share: 61.4,
            fill: 1.0,
            taken: 112_700,
            shield_block: 41,
            parry: 1_017,
            records: 12,
            attacks: 1_200,
        };
        let cells = |row: &PartyRow, keys: &[&str], round: bool| -> Vec<String> { keys.iter().map(|k| party_cell(row, k, round)).collect() };
        assert_eq!(cells(&row, &HEAL_COLUMNS, true), ["0.47M", "6.5K", "61%", "0.11M"]);
        assert_eq!(cells(&row, &TANK_COLUMNS, false)[1..], ["6,504", "61%", "41", "1,017"]);
        let none = PartyRow { per_second: None, ..row.clone() };
        assert_eq!(party_cell(&none, "dtps", true), "-", "no fight time");

        // No heal entry and no attack received: nothing behind the numbers.
        let idle = PartyRow { amount: 0, per_second: Some(0.0), share: 0.0, taken: 0, shield_block: 0, parry: 0, records: 0, attacks: 0, ..row.clone() };
        assert_eq!(cells(&idle, &HEAL_COLUMNS, true), ["-", "-", "-", "-"]);
        assert_eq!(cells(&idle, &TANK_COLUMNS, true), ["-", "-", "-", "-", "-"]);
        // Every attack blocked: 0 taken behind 5 attacks is a true 0.
        let wall = PartyRow { records: 5, attacks: 5, shield_block: 5, ..idle.clone() };
        assert_eq!(cells(&wall, &TANK_COLUMNS, true), ["0", "0", "0.0%", "5", "0"]);
        // Heal entries that healed nothing: a true 0.
        let full = PartyRow { records: 2, ..idle.clone() };
        assert_eq!(cells(&full, &HEAL_COLUMNS, true)[..3], ["0", "0", "0.0%"]);
        // Only damage over time ticks received: no attack, but damage.
        let ticks = PartyRow { amount: 900, taken: 900, ..idle };
        assert_eq!(cells(&ticks, &TANK_COLUMNS, true)[0], "900");
        assert_eq!(cells(&ticks, &HEAL_COLUMNS, true)[3], "900");
    }

    #[test]
    fn each_dps_column_has_its_number() {
        let mut meter = DpsData::new();
        let mut p = player("a", JobClass::Templar, 980_000.0, 7_300.0);
        p.active_dps = 8_150.0;
        p.last10_dps = 12_400.0;
        p.last30_dps = 9_900.0;
        p.last60_dps = 0.0;
        p.hits = 120;
        p.crit_hits = 45;
        p.max_hit = 61_234;
        meter.map.insert(11, p);
        meter.map.insert(12, player("b", JobClass::Sorcerer, 20_000.0, 300.0));
        let shown = MeterView::new().update(&meter, "", 5, &ViewOptions::default()).clone();
        let row = &Snapshot::new(&meter, &shown, 5).rows[0];
        let cells = |round: bool| -> Vec<String> {
            DPS_COLUMNS.iter().map(|key| row.cell(key, round)).collect()
        };
        assert_eq!(
            cells(true),
            ["0.98M", "7.3K", "8.2K", "98%", "38%", "12.4K", "9.9K", "0", "61.2K", "120"]
        );
        // Round DPS off: every rate whole, the rest as before.
        assert_eq!(
            cells(false),
            ["0.98M", "7,300", "8,150", "98%", "38%", "12,400", "9,900", "0", "61.2K", "120"]
        );
        let none = &Snapshot::new(&meter, &shown, 5).rows[1];
        assert_eq!(none.cell("crit", true), "-", "no hits, no crit rate");
    }

    #[test]
    fn a_player_with_only_ticks_shows_no_hit_numbers() {
        let mut meter = DpsData::new();
        meter.map.insert(11, player("a", JobClass::Ranger, 50_000.0, 900.0));
        let shown = MeterView::new().update(&meter, "", 5, &ViewOptions::default()).clone();
        let row = &Snapshot::new(&meter, &shown, 5).rows[0];
        assert!(row.ticks_only());
        for key in ["crit", "maxHit", "hits"] {
            assert_eq!(row.cell(key, true), "-", "{key}");
        }
        assert_eq!(row.cell("total", true), "50.0K", "the damage still shows");
        // A hit that did 0: the meter has it, so 0 stays.
        let mut hit = row.clone();
        hit.hits = 1;
        assert!(!hit.ticks_only());
        assert_eq!((hit.cell("maxHit", true), hit.cell("hits", true), hit.cell("crit", true)), ("0".into(), "1".into(), "0%".into()));
        // No damage at all: no hits, but nothing to say about ticks either.
        let empty = Row { damage: 0, ..row.clone() };
        assert!(!empty.ticks_only());
    }
}
