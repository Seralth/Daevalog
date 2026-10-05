//! Replaying a packet log file into the meter.

use super::AppState;

#[tauri::command]
pub(super) async fn replay_file(state: tauri::State<'_, AppState>, file_path: String) -> Result<String, String> {
    // Keep the live fights, then reset existing data before replay
    {
        let mut calc = state.dps_calculator.lock();
        for record in calc.snapshot_boss_fights_force() {
            let _ = state.fight_history.save_fight(&record);
        }
        calc.restart_target_selection(true);
    }
    state.data_storage.reset_nicknames();
    state.data_storage.forget_summon_links();

    // Feed packets directly to StreamProcessor, bypassing CaptureDispatcher
    // (no AION2 window check, no port detection needed for replay)
    let data_storage = state.data_storage.clone();
    let skill_lookup = state.skill_lookup.clone();
    let npc_lookup = state.npc_lookup.clone();
    let i18n_dir = state.i18n_data_dir.clone();

    let count = tokio::task::spawn_blocking(move || {
        use crate::capture::stream_processor::StreamProcessor;

        let mut processor = StreamProcessor::new(data_storage.clone(), skill_lookup, npc_lookup);
        // Load DOT IDs
        if let Some(ref data_dir) = i18n_dir {
            let mut dot_ids = std::collections::HashSet::new();
            if let Ok(text) = std::fs::read_to_string(data_dir.join("dot_skill_ids.json")) {
                if let Ok(ids) = serde_json::from_str::<Vec<i32>>(&text) {
                    for id in ids { dot_ids.insert(id); }
                }
            }
            processor.set_dot_skill_ids(dot_ids);
        }

        // Each line in the replay file is a complete game payload — process directly
        // without TCP reassembly (the assembler would incorrectly concatenate payloads)
        let text = match std::fs::read_to_string(&file_path) {
            Ok(t) => t.trim_start_matches('\u{feff}').to_string(), // Strip BOM
            Err(e) => return Err(format!("Failed to read file: {}", e)),
        };

        let mut packet_count = 0;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') { continue; }
            let parts: Vec<&str> = line.splitn(3, '|').collect();
            if parts.len() != 3 { continue; }
            // Use capture-time timestamp from the row, not wall clock
            if let Some(ts) = parse_replay_timestamp(parts[0].trim()) {
                processor.set_override_timestamp(Some(ts));
            }
            let hex = parts[2];
            let data = match decode_replay_hex(hex) {
                Some(d) => d,
                None => continue,
            };
            packet_count += 1;
            processor.consume_stream(&data);
        }

        let dmg = data_storage.damage_generation();
        Ok(format!("Replay complete. {} packets, {} damage events.", packet_count, dmg))
    }).await.map_err(|e| format!("Replay task failed: {}", e))?;

    // Force snapshot boss fights from the replay
    {
        let mut calc = state.dps_calculator.lock();
        let records = calc.snapshot_boss_fights_force();
        let mut sorted = records;
        sorted.sort_by(|a, b| b.total_damage.cmp(&a.total_damage));
        for record in sorted.iter().take(10) {
            if let Err(e) = state.fight_history.save_fight(record) {
                tracing::warn!("Failed to save replay fight: {}", e);
            } else {
                tracing::info!("Saved replay fight: {} ({})", record.boss_name, record.id);
            }
        }
        // Mark all targets as saved so the periodic auto-save loop doesn't re-process them
        calc.mark_all_targets_saved();
    }

    count
}

/// Parse an ISO 8601 timestamp (or plain epoch millis) into epoch milliseconds.
fn parse_replay_timestamp(s: &str) -> Option<i64> {
    // Try plain integer first (epoch millis)
    if let Ok(ms) = s.parse::<i64>() {
        return Some(ms);
    }
    // Parse ISO 8601: "2026-04-01T14:08:18.447814200-03:00"
    // Manual parse to avoid adding a chrono dependency
    // Format: YYYY-MM-DDTHH:MM:SS.fractional[+-]HH:MM
    let t_pos = s.find('T')?;
    let date_part = &s[..t_pos];
    let time_and_tz = &s[t_pos + 1..];

    let date_parts: Vec<&str> = date_part.split('-').collect();
    if date_parts.len() != 3 { return None; }
    let year: i64 = date_parts[0].parse().ok()?;
    let month: i64 = date_parts[1].parse().ok()?;
    let day: i64 = date_parts[2].parse().ok()?;

    // Split time from timezone offset (look for + or - after the seconds)
    let (time_part, tz_offset_mins) = if let Some(plus_pos) = time_and_tz.rfind('+') {
        if plus_pos > 6 { // Must be after HH:MM:SS
            let tz = &time_and_tz[plus_pos + 1..];
            let tz_parts: Vec<&str> = tz.split(':').collect();
            let h: i64 = tz_parts.first()?.parse().ok()?;
            let m: i64 = tz_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            (&time_and_tz[..plus_pos], h * 60 + m)
        } else {
            (time_and_tz, 0i64)
        }
    } else if let Some(minus_pos) = time_and_tz.rfind('-') {
        if minus_pos > 6 {
            let tz = &time_and_tz[minus_pos + 1..];
            let tz_parts: Vec<&str> = tz.split(':').collect();
            let h: i64 = tz_parts.first()?.parse().ok()?;
            let m: i64 = tz_parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            (&time_and_tz[..minus_pos], -(h * 60 + m))
        } else {
            (time_and_tz, 0i64)
        }
    } else {
        // No timezone, treat as UTC
        let tp = time_and_tz.trim_end_matches('Z');
        (tp, 0i64)
    };

    // Parse time: HH:MM:SS.fractional
    let colon_parts: Vec<&str> = time_part.split(':').collect();
    if colon_parts.len() < 3 { return None; }
    let hour: i64 = colon_parts[0].parse().ok()?;
    let minute: i64 = colon_parts[1].parse().ok()?;
    let sec_parts: Vec<&str> = colon_parts[2].split('.').collect();
    let second: i64 = sec_parts[0].parse().ok()?;
    let millis: i64 = if sec_parts.len() > 1 {
        let frac = sec_parts[1];
        // Take first 3 digits for milliseconds
        let padded = if frac.len() >= 3 { &frac[..3] } else { frac };
        let mut ms: i64 = padded.parse().ok()?;
        if frac.len() < 3 {
            for _ in 0..(3 - frac.len()) { ms *= 10; }
        }
        ms
    } else {
        0
    };

    // Convert to Unix epoch using a simplified algorithm
    // Days from epoch (1970-01-01)
    let days = days_from_civil(year, month, day);
    let total_secs = days * 86400 + hour * 3600 + minute * 60 + second - tz_offset_mins * 60;
    Some(total_secs * 1000 + millis)
}

/// Days from 1970-01-01 for a given civil date (Howard Hinnant's algorithm).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let m_adj = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * m_adj + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

fn decode_replay_hex(hex: &str) -> Option<Vec<u8>> {
    let clean: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.len() % 2 != 0 { return None; }
    let mut bytes = Vec::with_capacity(clean.len() / 2);
    for chunk in clean.as_bytes().chunks(2) {
        let h = match chunk[0] {
            b'0'..=b'9' => chunk[0] - b'0',
            b'a'..=b'f' => chunk[0] - b'a' + 10,
            b'A'..=b'F' => chunk[0] - b'A' + 10,
            _ => return None,
        };
        let l = match chunk[1] {
            b'0'..=b'9' => chunk[1] - b'0',
            b'a'..=b'f' => chunk[1] - b'a' + 10,
            b'A'..=b'F' => chunk[1] - b'A' + 10,
            _ => return None,
        };
        bytes.push((h << 4) | l);
    }
    Some(bytes)
}
