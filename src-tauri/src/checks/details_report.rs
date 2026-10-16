//! Replays a capture and prints, at the end of each fight, what Details
//! shows (`combat/details_view.rs`) for a few players and both the fight's
//! target and every target, with the Details context and every backend answer
//! the view was built from.
//!
//! ```text
//! A2_REPLAY_FILE=packets_x.txt   the capture (required)
//! A2_DETAILS_MODE=encounter      the meter's mode (default bossTargets)
//! A2_DETAILS_MAX=4               fights reported (default 10)
//! cargo test --lib details_report -- --ignored --nocapture
//! ```
//!
//! A fight ends once its target and fight time have stood for 8 s of capture
//! time; the meter's reading at the capture's end is reported too. Each
//! fight is a `DCTX {"t", "mode", "you", "context", "calls"}` line, `calls`
//! holding each call with its answer, then one `DVIEW {"t", "selection",
//! "view"}` line per selection: every player, you, the two others with the
//! most damage and the one with the most healing, each on the meter's target
//! and on every target, and each player who received damage on the target
//! Details opens on from the meter. Then one `DPARTY {"t", "target",
//! "light", "tanks", "party"}` line: the meter's Heal and Tank tabs on that target,
//! every row, and the damage received per player they come from. Last, one
//! `DHOVER {"t", "player", "answer", "skills"}` line per player: the meter's
//! hover card for that player's row, and the answer 1.x reads it from. And
//! one `DSUMS {"t", "heal", "healBad", "taken", "takenBad", "views",
//! "targetsBad"}` line: the heal and damage received entries and views
//! whose ticks were checked against their totals, and how many did not add
//! up (the run stops on one).

use std::collections::HashSet;
use std::io::Write;
use std::sync::Arc;

use serde_json::json;

use crate::capture::packet_log::{LogLine, PacketLogReplay};
use crate::capture::ping_tracker::PingTracker;
use crate::combat::data_storage::{DataStorage, UNATTRIBUTED_ID};
use crate::combat::details_view::{Call, DetailsView, Selection, Words};
use crate::combat::dps_calculator::DpsCalculator;
use crate::combat::meter_view::{self, ViewOptions};
use crate::entity::detail_bundle::TargetBreakdownReply;

const TICK_MS: i64 = 100;
/// A fight has ended when its time has not moved for this long.
const QUIET_MS: i64 = 8_000;
/// Shorter fights are left out.
const MIN_FIGHT_MS: i64 = 5_000;

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

fn call_json(call: &Call) -> serde_json::Value {
    match call {
        Call::Target { target, actors } => json!({ "target": target, "actors": actors }),
        Call::Displayed { actors } => json!({ "displayed": true, "actors": actors }),
    }
}

/// Prints one fight: the context, the views and the answers behind them.
fn report(meter: &DpsCalculator, storage: &DataStorage, at: i64, mode: &str, out: &mut dyn Write) {
    let ctx = meter.get_details_context();
    let words = Words::default();
    let mut calls: Vec<(Call, TargetBreakdownReply)> = Vec::new();
    let mut ask = |call: &Call| -> TargetBreakdownReply {
        if let Some((_, answer)) = calls.iter().find(|(c, _)| c == call) {
            return answer.clone();
        }
        let answer = meter.details_call(call);
        calls.push((call.clone(), answer.clone()));
        answer
    };
    let meter_target = storage.focused_target();
    let targets: Vec<Option<i32>> = if meter_target > 0 { vec![Some(meter_target), None] } else { vec![None] };

    let everyone = meter.details_view_from(&ctx, &Selection { player: None, target: None }, &words, &mut ask);
    let you = storage.local_player_id().and_then(|id| i32::try_from(id).ok()).filter(|id| ctx.actors.iter().any(|a| a.actor_id == *id));
    let mut players: Vec<Option<i32>> = vec![None];
    players.extend(you.map(Some));
    let others = everyone.players.iter().filter(|p| p.amount > 0 && Some(p.id) != you && p.id != UNATTRIBUTED_ID).take(2);
    players.extend(others.map(|p| Some(p.id)));
    if let Some(healer) = everyone.healers.first().filter(|h| !players.contains(&Some(h.player.id))) {
        players.push(Some(healer.player.id));
    }

    let mut views: Vec<(Selection, DetailsView)> = Vec::new();
    for &target in &targets {
        for &player in &players {
            let selection = Selection { player, target };
            let view = if selection == (Selection { player: None, target: None }) {
                everyone.clone()
            } else {
                meter.details_view_from(&ctx, &selection, &words, &mut ask)
            };
            views.push((selection, view));
        }
    }

    // The meter's Heal and Tank tabs, on the target Details opens on from
    // the meter, and Details on each player they list with damage received.
    let opening = meter.details_opening_target(&ctx, meter_target, meter_target > 0, true);
    let party_selection = Selection { player: None, target: opening };
    let party_view = match views.iter().find(|(s, _)| *s == party_selection) {
        Some((_, view)) => view.clone(),
        None => {
            let view = meter.details_view_from(&ctx, &party_selection, &words, &mut ask);
            views.push((party_selection, view.clone()));
            view
        }
    };
    for tank in &party_view.tanks {
        let selection = Selection { player: Some(tank.player.id), target: opening };
        if !views.iter().any(|(s, _)| *s == selection) {
            let view = meter.details_view_from(&ctx, &selection, &words, &mut ask);
            views.push((selection, view));
        }
    }
    // The meter reads the tabs the light way: it must give the full view's.
    let light = meter.party_view_from(&meter.party_context(), opening);
    let same = light.players == party_view.players
        && light.healers == party_view.healers
        && light.tanks == party_view.tanks
        && light.battle_ms == party_view.heal.stats.battle_ms;
    let every_row = ViewOptions { player_limit: usize::MAX, ..ViewOptions::default() };
    let party = meter_view::party(
        &light.players,
        &light.healers,
        &light.tanks,
        light.battle_ms,
        // Every row: TRAIN keeps only yours, which the page has no rule for.
        false,
        &every_row,
    );

    // The ticks the 2.0 charts and heal targets are drawn from add up to the
    // numbers beside them: each heal entry's to its healing and tick count,
    // each damage received entry's to its damage, each view's heal targets to
    // its healing. Never written, so only here can they be checked.
    let answers = || calls.iter().map(|(_, a)| a);
    let (mut heal, mut heal_bad, mut taken, mut taken_bad, mut targets_bad) = (0, 0, 0, 0, 0);
    for e in ctx.heal_skills.iter().chain(answers().flat_map(|a| a.heal_skills.iter())) {
        heal += 1;
        let sum: i64 = e.heal_ticks.iter().map(|t| t.amount).sum();
        if sum != e.dmg || e.heal_ticks.len() != e.time as usize {
            heal_bad += 1;
        }
    }
    for e in ctx.taken_skills.iter().chain(answers().flat_map(|a| a.taken_skills.iter())) {
        taken += 1;
        if e.hit_times.iter().map(|h| h.1).sum::<i64>() != e.stats.damage {
            taken_bad += 1;
        }
    }
    for (_, view) in &views {
        if view.heal.targets.iter().map(|t| t.amount).sum::<i64>() != view.heal.stats.heal {
            targets_bad += 1;
        }
    }
    writeln!(out, "DSUMS {}", json!({ "t": at, "heal": heal, "healBad": heal_bad, "taken": taken, "takenBad": taken_bad, "views": views.len(), "targetsBad": targets_bad })).unwrap();
    assert_eq!((heal_bad, taken_bad, targets_bad), (0, 0, 0), "ticks that do not add up at {at}");

    let calls: Vec<serde_json::Value> = calls.iter().map(|(c, a)| json!({ "call": call_json(c), "answer": a })).collect();
    writeln!(out, "DCTX {}", json!({ "t": at, "mode": mode, "you": you, "context": ctx, "calls": calls })).unwrap();
    for (selection, view) in views {
        let selection = json!({ "player": selection.player, "target": selection.target });
        writeln!(out, "DVIEW {}", json!({ "t": at, "selection": selection, "view": view })).unwrap();
    }
    writeln!(out, "DPARTY {}", json!({ "t": at, "target": opening, "light": same, "tanks": light.tanks, "party": party })).unwrap();
    for player in light.players.iter().filter(|p| !p.unattributed) {
        let answer = meter.get_displayed_hover_details(Some(&[player.id]));
        let (skills, _) = meter.hover_of(&answer, &words);
        writeln!(out, "DHOVER {}", json!({ "t": at, "player": player.id, "answer": answer, "skills": skills })).unwrap();
    }
}

#[test]
#[ignore]
fn details_report() {
    let file = env("A2_REPLAY_FILE").expect("A2_REPLAY_FILE");
    let mode = env("A2_DETAILS_MODE").unwrap_or_else(|| "bossTargets".into());
    let max: usize = env("A2_DETAILS_MAX").and_then(|v| v.parse().ok()).unwrap_or(10);
    let (skills, npcs, dot_ids) = super::lookups();
    let storage = Arc::new(DataStorage::new());
    let mut feed = PacketLogReplay::new(storage.clone(), skills.clone(), npcs.clone(), dot_ids);
    let mut meter = DpsCalculator::new(storage.clone(), skills, npcs, Arc::new(PingTracker::new()));
    meter.set_target_selection_mode(&mode);

    let text = std::fs::read_to_string(&file).expect("the capture");
    let out = std::io::stdout();
    let mut out = std::io::BufWriter::new(out.lock());
    let mut reported: HashSet<(i32, i64)> = HashSet::new();
    // The fight on the meter, its time, and since when that time has stood.
    let mut standing: Option<(i32, i64, i64)> = None;
    let mut next_tick: Option<i64> = None;
    let mut last_payload = None;
    for line in text.lines() {
        let Some(line) = LogLine::parse(line) else { continue };
        let tick = next_tick.get_or_insert(line.at_ms);
        while *tick <= line.at_ms {
            let at = *tick;
            crate::clock::set_override(Some(at));
            let reading = meter.get_dps();
            let fight = (reading.target_id, reading.battle_time);
            match standing {
                Some((target, time, since)) if (target, time) == fight => {
                    if time >= MIN_FIGHT_MS && at - since >= QUIET_MS && reported.len() < max && reported.insert((target, time)) {
                        // The storage the screen reads, as the app's Details does.
                        report(&meter, &meter.details_source().storage(), at, &mode, &mut out);
                    }
                }
                _ => standing = Some((fight.0, fight.1, at)),
            }
            *tick += TICK_MS;
        }
        feed.feed(&line);
        last_payload = Some(line.at_ms);
    }
    if let Some(at) = last_payload {
        crate::clock::set_override(Some(at));
        let reading = meter.get_dps();
        if reading.battle_time > 0 && !reported.contains(&(reading.target_id, reading.battle_time)) {
            report(&meter, &meter.details_source().storage(), at, &mode, &mut out);
        }
    }
    crate::clock::set_override(None);
}
