//! Player records (`45 36`, `44 36`): a player's id, name, class and home
//! server, from the record the server sends when a player comes into view.

use crate::capture::abnormal::NEVER;
use crate::capture::decoder::{listed_server, player_spawn_at, PlayerSpawn};
use crate::capture::opcodes::{PLAYER_SPAWN, PLAYER_SPAWN_OLD};
use crate::capture::varint::{u16_at, u32_at};
use crate::entity::job_class::JobClass;

/// What one player record states.
#[derive(Debug, Clone, PartialEq)]
pub struct PlayerRecord {
    pub id: i32,
    pub name: String,
    /// The u32 after the name, in the roster's encoding (as in your own
    /// record, `33 36`, which has your server in between). It agrees with
    /// the class skills a player uses and with the roster.
    pub class: Option<JobClass>,
    pub server: Option<u16>,
}

/// Every named player record anywhere in `data`, at any offset, as the
/// meter's own spawn reader reads the id and name.
pub fn player_records(data: &[u8]) -> Vec<PlayerRecord> {
    let mut found = Vec::new();
    for i in 0..data.len().saturating_sub(8) {
        let op = [data[i], data[i + 1]];
        if op != PLAYER_SPAWN && op != PLAYER_SPAWN_OLD {
            continue;
        }
        let Some(PlayerSpawn::Named(id, name, field)) = player_spawn_at(data, i + 2) else { continue };
        let class = u32_at(data, field.end).and_then(JobClass::job_from_party_list);
        found.push(PlayerRecord { id, name, class, server: record_server(data, field.end) });
    }
    found
}

/// The end of the fixed block that closes a player record: eight `ff`, then
/// the end of an abnormal that never ends.
const RECORD_TAIL: [u8; 16] = {
    let n = NEVER.to_le_bytes();
    [0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, n[0], n[1], n[2], n[3], n[4], n[5], n[6], n[7]]
};

/// The home server in a player record, from its bytes after the name.
///
/// It agrees with the party roster's server (cross-server members
/// included) and with the loot and summon records'. A player without a
/// legion has it 9 or 10 bytes before the closing block, then a byte and
/// `10` or `11`; a legion member has zeros there and the server in the
/// legion block, `00 00 <server u16> <len> <legion name>`. Some records have
/// neither.
fn record_server(data: &[u8], after: usize) -> Option<u16> {
    let window = data.get(after..data.len().min(after + 512))?;
    let end = window.windows(RECORD_TAIL.len()).position(|w| w == RECORD_TAIL)?;
    let server = |i: usize| u16_at(window, i).filter(|&s| listed_server(s));
    for back in [9, 10] {
        if end >= back && matches!(window[end - back + 3], 0x10 | 0x11) {
            if let Some(s) = server(end - back) {
                return Some(s);
            }
        }
    }
    (0..end.saturating_sub(5))
        .filter(|&j| window[j] == 0 && window[j + 1] == 0 && server(j + 2).is_some())
        .filter(|&j| {
            let len = usize::from(window[j + 4]);
            let name = window.get(j + 5..j + 5 + len).filter(|_| (1..=40).contains(&len) && j + 5 + len <= end);
            name.and_then(|n| std::str::from_utf8(n).ok())
                .is_some_and(|n| n.chars().all(|c| c.is_alphanumeric() || c == ' '))
        })
        .last()
        .and_then(|j| server(j + 2))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::player_record;

    /// What follows a player record's name: class, four bytes, `tail`, the
    /// closing block and two bytes.
    fn after_name(class: u32, tail: &[u8]) -> Vec<u8> {
        [&class.to_le_bytes()[..], &[0x01, 0x02, 0x00, 0x00], tail, &RECORD_TAIL, &[0x00, 0x00]].concat()
    }

    /// The two layouts a player record comes in, a player with no legion
    /// (server before the closing block) and a legion member (server in the
    /// legion block), and one whose class and server do not read. 1201 =
    /// `b1 04`; class 30 is a Cleric, 6 a Gladiator.
    #[test]
    fn records_give_id_name_class_and_server() {
        let plain = player_record(9001, Some("Tester"), &after_name(30, &[0xb1, 0x04, 0x22, 0x11, 0x01, 0x02, 0x03, 0x04, 0x05]));
        let legion_block = [0, 0, 0, 0, 0, 0, 0xb2, 0x04, 0x05, b'G', b'u', b'i', b'l', b'd', 0x01, 0, 0, 0, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07];
        let legion = player_record(9002, Some("Other"), &after_name(6, &legion_block));
        let blank = player_record(9003, Some("Third"), &after_name(0, &[0x00, 0x00, 0x22, 0x11, 0x01, 0x02, 0x03, 0x04, 0x05]));
        let want = [
            PlayerRecord { id: 9001, name: "Tester".into(), class: Some(JobClass::Cleric), server: Some(1201) },
            PlayerRecord { id: 9002, name: "Other".into(), class: Some(JobClass::Gladiator), server: Some(1202) },
            PlayerRecord { id: 9003, name: "Third".into(), class: None, server: None },
        ];
        assert_eq!(player_records(&[plain, legion, blank].concat()), want);
    }

    #[test]
    fn a_record_without_its_name_bit_or_with_a_bad_name_is_left_out() {
        assert!(player_records(&player_record(9001, None, &after_name(30, &[]))).is_empty());
        assert!(player_records(&player_record(9001, Some("Te-ster"), &after_name(30, &[]))).is_empty());
    }
}
