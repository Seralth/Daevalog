//! Every captured payload comes through here. Until the game's connection is
//! known, connections that carry the game's record marker are counted, and
//! the first at the game's rate is locked (`PortWatch`). From then
//! on that connection's server-to-client bytes go to the parser, the packet
//! log and the ring, in TCP order, each byte once; and when the game moves
//! to another connection, the lock follows it (`device_follow`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use tracing::info;

use super::tcp_chunk::TcpChunk;
use super::port_watch::PortWatch;
use super::device_follow::{same_device, Follow, Next};
use super::ping_tracker::PingTracker;
use super::reassembly::Reassembler;
use super::decoder::PacketDecoder;
use super::tcp_order::TcpOrder;
use crate::combat::data_storage::DataStorage;
use crate::i18n::lookup::{NpcLookup, SkillLookup};

/// The game server also talks TLS from its port, on a second connection. A
/// connection whose first segment is a TLS record is left out whole: of the
/// log, the slices and the parser. Decided once per connection, so no byte
/// inside the game's stream is ever skipped.
#[derive(Default)]
struct ConnectionFilter {
    seen: std::collections::HashSet<Connection>,
    tls: std::collections::HashSet<Connection>,
}

/// One direction of a connection: source, then destination.
type Flow = (Option<String>, u16, Option<String>, u16);

/// Both ends of a connection, address and port, the lower end first.
type Connection = [(Option<std::net::IpAddr>, u16); 2];

impl ConnectionFilter {
    fn admit(&mut self, cap: &TcpChunk) -> bool {
        let ip = |ip: &Option<String>| ip.as_deref().and_then(|s| s.parse().ok());
        let mut connection = [(ip(&cap.src_ip), cap.src_port), (ip(&cap.dst_ip), cap.dst_port)];
        connection.sort();
        if self.seen.insert(connection) && is_tls_record(&cap.data) {
            tracing::info!("Connection {} -> {} carries TLS: not the game, left out", cap.src_port, cap.dst_port);
            self.tls.insert(connection);
        }
        !self.tls.contains(&connection)
    }

    fn clear(&mut self) {
        self.seen.clear();
        self.tls.clear();
    }
}

/// The game ends its records with `0E 00 36` since its June 2026 update, and
/// with `06 00 36` before it.
const RECORD_ENDS: [[u8; 3]; 2] = [[0x0E, 0x00, 0x36], [0x06, 0x00, 0x36]];

/// The game's rate: this many marked payloads on one connection, none more
/// than `RATE_GAP_MS` after the one before. The game sends about 20 a second,
/// so the 12th comes well within a second of the first.
const RATE_COUNT: u32 = 12;
const RATE_GAP_MS: i64 = 3_000;

/// How often to look for the game's process while it is missing, and while
/// it runs.
const LOOK_MISSING_MS: i64 = 10_000;
const LOOK_RUNNING_MS: i64 = 60_000;

/// A locked connection that gave the parser nothing for longer than this is
/// let go, and the search starts again.
const SILENT_LOCK_MS: i64 = 120_000;

/// While nothing is locked, what capture saw goes to the log this often.
const TALLY_EVERY_MS: i64 = 30_000;
/// Devices named in that log line, the busiest first.
const TALLY_DEVICES: usize = 8;

/// The capture's device list is read at most this often, to tell whether the
/// picked device is there.
const DEVICE_LIST_MS: i64 = 1_000;

/// A TLS record holds at most 2^14 bytes plus 256 (RFC 8446, 5.2).
const TLS_LONGEST: u16 = (1 << 14) + 256;

/// Starts like a TLS record: content type 20 to 23, then version 3.0 to 3.4.
fn tls_start(data: &[u8]) -> bool {
    matches!(data, [0x14..=0x17, 0x03, 0x00..=0x04, ..])
}

/// A whole TLS record header, with a length a record may have.
fn is_tls_record(data: &[u8]) -> bool {
    match data {
        [_, _, _, high, low, ..] if tls_start(data) => (1..=TLS_LONGEST).contains(&u16::from_be_bytes([*high, *low])),
        _ => false,
    }
}

/// Holds one of the game's record ends anywhere.
fn marked(data: &[u8]) -> bool {
    data.windows(3).any(|three| RECORD_ENDS.iter().any(|end| end == three))
}

/// A connection by its two ports, the lower first.
fn ports_of(cap: &TcpChunk) -> (u16, u16) {
    (cap.src_port.min(cap.dst_port), cap.src_port.max(cap.dst_port))
}

/// Whether a payload on `device` is on the locked device. With no locked
/// device, every payload is.
fn on_locked_device(locked: Option<&str>, device: Option<&str>) -> bool {
    locked.is_none() || same_device(locked, device)
}

/// What capture saw while nothing was locked.
#[derive(Default)]
struct Tally {
    /// Per device label: payloads, and how many of them were marked.
    devices: HashMap<String, (u64, u64)>,
    /// Payloads let go because the game was not running.
    no_game: u64,
}

impl Tally {
    fn add(&mut self, cap: &TcpChunk, game_running: bool) {
        let device = cap.device_name.clone().unwrap_or_else(|| "?".to_string());
        let (all, hits) = self.devices.entry(device).or_default();
        *all += 1;
        *hits += u64::from(marked(&cap.data));
        if !game_running {
            self.no_game += 1;
        }
    }

    /// The busiest devices first, at most `TALLY_DEVICES`.
    fn busiest(&self) -> Vec<(&str, u64, u64)> {
        let mut rows: Vec<(&str, u64, u64)> = self.devices.iter().map(|(name, &(all, hits))| (name.as_str(), all, hits)).collect();
        rows.sort_by_key(|row| std::cmp::Reverse(row.1));
        rows.truncate(TALLY_DEVICES);
        rows
    }

    fn line(&self, span_ms: i64, game_running: bool, look_alikes: &[String]) -> String {
        let mut line = format!(
            "No game connection locked in the last {} s. Game {}",
            span_ms / 1_000,
            if game_running { "running" } else { "not found" }
        );
        if self.no_game > 0 {
            line.push_str(&format!(", {} payloads ignored for it", self.no_game));
        }
        if !game_running {
            if look_alikes.is_empty() {
                line.push_str(". No process mentions \"aion\"");
            } else {
                line.push_str(&format!(". Processes like it: {}", look_alikes.join("; ")));
            }
        }
        for (device, all, hits) in self.busiest() {
            line.push_str(&format!(". {device}: {all} payloads, {hits} marked"));
        }
        line
    }
}

/// What the dispatcher asks of the system around it. Tests answer instead.
trait World {
    /// The game's window title, `None` while it is not running.
    fn game_title(&mut self) -> Option<String>;
    /// Processes that look a little like the game, for the log.
    fn look_alikes(&mut self) -> Vec<String>;
    /// The capture's device labels, `None` before the first list.
    fn device_list(&mut self) -> Option<Vec<String>>;
    /// A payload as the parser gets it, for the packet log and the ring.
    /// `you` is your character, once the game's record named it.
    fn record(&mut self, cap: &TcpChunk, you: Option<&str>);
}

struct LiveWorld;

impl World for LiveWorld {
    fn game_title(&mut self) -> Option<String> {
        crate::platform::window_detector::find_aion2_window_title()
    }

    fn look_alikes(&mut self) -> Vec<String> {
        crate::platform::window_detector::describe_candidates()
    }

    fn device_list(&mut self) -> Option<Vec<String>> {
        super::live::listed_devices()
    }

    fn record(&mut self, cap: &TcpChunk, you: Option<&str>) {
        crate::logging::logger::write_segment_line(cap, you);
        crate::share::ring::record(cap);
    }
}

pub struct PacketRouter {
    /// What every parser made gets: the skills whose damage comes over
    /// time, the shared storage and the names.
    storage: Arc<DataStorage>,
    skills: Arc<SkillLookup>,
    dot_ids: HashSet<i32>,
    npcs: Arc<NpcLookup>,
    ports: Arc<PortWatch>,
    ping: Arc<PingTracker>,
}

impl PacketRouter {
    pub fn new(
        data_storage: Arc<DataStorage>,
        skill_lookup: Arc<SkillLookup>,
        npc_lookup: Arc<NpcLookup>,
        port_detector: Arc<PortWatch>,
        ping_tracker: Arc<PingTracker>,
    ) -> Self {
        Self {
            dot_ids: HashSet::new(),
            ping: ping_tracker,
            ports: port_detector,
            npcs: npc_lookup,
            skills: skill_lookup,
            storage: data_storage,
        }
    }

    /// The skills whose damage comes over time, for every parser made.
    pub fn use_tick_skills(&mut self, ids: HashSet<i32>) {
        self.dot_ids = ids;
    }

    /// Handles payloads until the channel closes.
    pub async fn run(&self, mut receiver: tokio::sync::mpsc::Receiver<TcpChunk>) {
        let mut state = DispatchState::new(self, crate::clock::now_ms());
        while let Some(cap) = receiver.recv().await {
            state.feed(crate::clock::now_ms(), cap, &mut LiveWorld);
        }
    }
}

/// The dispatch thread's working state.
struct DispatchState<'a> {
    dispatcher: &'a PacketRouter,
    filter: ConnectionFilter,
    follow: Follow,
    /// One parser per connection.
    parsers: HashMap<(u16, u16), (Reassembler, PacketDecoder)>,
    /// One TCP order per direction of the locked connection.
    orders: HashMap<Flow, TcpOrder>,
    /// Per connection before a lock: marked payloads in a row, and when the
    /// last one came.
    rates: HashMap<(u16, u16), (u32, i64)>,
    /// Payloads a follow kept, handled before the next from the channel.
    backlog: VecDeque<TcpChunk>,
    /// Whether the game ran at the last look; `None` before the first.
    game: Option<bool>,
    looked_ms: i64,
    tally: Tally,
    tally_since_ms: i64,
    /// The locked port at the payload before.
    port_before: Option<u16>,
    devices: Option<Vec<String>>,
    devices_read_ms: Option<i64>,
}

impl<'a> DispatchState<'a> {
    fn new(dispatcher: &'a PacketRouter, now: i64) -> Self {
        Self {
            dispatcher,
            filter: ConnectionFilter::default(),
            follow: Follow::new(now, RATE_COUNT, RATE_GAP_MS),
            parsers: HashMap::new(),
            orders: HashMap::new(),
            rates: HashMap::new(),
            backlog: VecDeque::new(),
            game: None,
            looked_ms: now,
            tally: Tally::default(),
            tally_since_ms: now,
            port_before: None,
            devices: None,
            devices_read_ms: None,
        }
    }

    /// One payload from the channel, then whatever a follow kept.
    fn feed(&mut self, now: i64, cap: TcpChunk, world: &mut dyn World) {
        self.backlog.push_back(cap);
        while let Some(next) = self.backlog.pop_front() {
            self.handle(now, next, world);
        }
    }

    fn handle(&mut self, now: i64, cap: TcpChunk, world: &mut dyn World) {
        if !self.look_for_game(now, world) {
            self.tally_unlocked(now, &cap, false, world);
            return;
        }
        // Tallied before the silent check, so the payload that ends a lock
        // counts as one seen while locked.
        self.tally_unlocked(now, &cap, true, world);
        self.let_go_if_silent(now);
        let lock = self.dispatcher.ports.locked();
        let port = lock.as_ref().map(|(_, port)| *port);
        if self.port_before.is_some() && self.port_before != port {
            // The lock ended or moved: connections are judged again.
            self.filter.clear();
            self.orders.clear();
        }
        self.port_before = port;
        let pick = self.dispatcher.ports.picked();
        let pick_absent = self.pick_absent(now, pick.as_deref(), world);
        self.follow.sync(now, pick.as_deref(), lock.as_ref().map(|(device, port)| (device.as_deref(), *port)));
        match lock {
            None => self.before_lock(now, &cap, pick_absent, world),
            Some((device, port)) => self.while_locked(now, cap, device.as_deref(), port, pick_absent, world),
        }
    }

    /// Whether the game runs, looking again when the last look is old enough.
    fn look_for_game(&mut self, now: i64, world: &mut dyn World) -> bool {
        let wait = if self.game == Some(true) { LOOK_RUNNING_MS } else { LOOK_MISSING_MS };
        if let Some(running) = self.game
            && now - self.looked_ms < wait
        {
            return running;
        }
        self.looked_ms = now;
        let title = world.game_title();
        let running = title.is_some();
        if self.game != Some(running) {
            match title {
                Some(title) => info!("Game found: {title}"),
                None => info!("Game not found (looking for the AION2 process): captured payloads are ignored until it runs"),
            }
            if self.game == Some(true) {
                self.start_over();
            }
            self.game = Some(running);
        }
        running
    }

    /// A lock, every parser, TCP order and rate count gone.
    fn start_over(&mut self) {
        self.dispatcher.ports.reset();
        self.dispatcher.ping.reset();
        self.forget_streams();
    }

    fn forget_streams(&mut self) {
        self.parsers.clear();
        self.orders.clear();
        self.rates.clear();
    }

    fn let_go_if_silent(&mut self, now: i64) {
        let ports = &self.dispatcher.ports;
        if ports.current_port().is_none() {
            return;
        }
        if let Some(parsed) = ports.last_parse()
            && now - parsed > SILENT_LOCK_MS
        {
            info!("Nothing parsed from the game's connection for {} s: looking for it again", (now - parsed) / 1_000);
            self.start_over();
        }
    }

    /// The picked device is listed as gone. Not listed yet counts as there.
    fn pick_absent(&mut self, now: i64, pick: Option<&str>, world: &mut dyn World) -> bool {
        let Some(pick) = pick else { return false };
        if self.devices_read_ms.is_none_or(|at| now - at >= DEVICE_LIST_MS) {
            self.devices = world.device_list();
            self.devices_read_ms = Some(now);
        }
        // The current pick on purpose, against the stored list: a new pick
        // acts at once, not at the next read.
        self.devices.as_ref().is_some_and(|list| !list.iter().any(|label| label.eq_ignore_ascii_case(pick)))
    }

    fn tally_unlocked(&mut self, now: i64, cap: &TcpChunk, game_running: bool, world: &mut dyn World) {
        if self.dispatcher.ports.current_port().is_some() {
            // The 30 s start again with every payload while locked, so no
            // report comes sooner than 30 s after a lock ends.
            self.tally_since_ms = now;
            return;
        }
        self.tally.add(cap, game_running);
        let span = now - self.tally_since_ms;
        if span >= TALLY_EVERY_MS {
            let look_alikes = if game_running { Vec::new() } else { world.look_alikes() };
            info!("{}", self.tally.line(span, game_running, &look_alikes));
            self.tally = Tally::default();
            self.tally_since_ms = now;
        }
    }

    /// Nothing locked: count the game's payloads per connection and lock the
    /// first connection at the game's rate.
    fn before_lock(&mut self, now: i64, cap: &TcpChunk, pick_absent: bool, world: &mut dyn World) {
        if !self.filter.admit(cap) || tls_start(&cap.data) || !marked(&cap.data) {
            return;
        }
        let pair = ports_of(cap);
        let count = match self.rates.get(&pair) {
            Some(&(count, last)) if now - last <= RATE_GAP_MS => count + 1,
            _ => 1,
        };
        self.rates.insert(pair, (count, now));
        // Parsed before the lock, so names and spawns before the first fight
        // are kept.
        self.deliver(now, cap, world);
        let device = cap.device_name.as_deref();
        let ports = &self.dispatcher.ports;
        ports.offer(cap.src_port, device, now);
        self.follow.note_game(now, device);
        // Markers travel server to client: the source port is the server's.
        if count >= RATE_COUNT && self.follow.may_lock(now, device, pick_absent) && ports.try_lock(cap.src_port, device, now) {
            self.parsers.retain(|key, _| *key == pair);
            self.rates.clear();
        }
    }

    /// Locked on `port` on `device`.
    fn while_locked(
        &mut self,
        now: i64,
        mut cap: TcpChunk,
        device: Option<&str>,
        port: u16,
        pick_absent: bool,
        world: &mut dyn World,
    ) {
        let on = on_locked_device(device, cap.device_name.as_deref()) && (cap.src_port == port || cap.dst_port == port);
        if !on {
            let game = marked(&cap.data) && !tls_start(&cap.data);
            if let Next::Follow { device, port, packets } = self.follow.elsewhere(now, cap, game, pick_absent) {
                self.dispatcher.ports.move_to(port, device.as_deref());
                self.dispatcher.ping.reset();
                self.forget_streams();
                for kept in packets.into_iter().rev() {
                    self.backlog.push_front(kept);
                }
            }
            return;
        }
        self.follow.note_locked_packet(now);
        // Ping first: requests go client to server, and a reply may come in
        // bytes the TCP order drops as a repeat.
        let ping = &self.dispatcher.ping;
        let shown = ping.current_ping_ms();
        ping.observe_segment(&cap, port);
        if ping.current_ping_ms() != shown {
            self.dispatcher.ports.saw_parse(now);
        }
        if cap.src_port != port || !self.filter.admit(&cap) {
            return;
        }
        if let Some(seq) = cap.tcp_seq {
            let flow: Flow = (cap.src_ip.clone(), cap.src_port, cap.dst_ip.clone(), cap.dst_port);
            let fresh = self.orders.entry(flow).or_default().accept(seq, &cap.data, now);
            if fresh.is_empty() {
                return;
            }
            cap.data = fresh;
        }
        self.deliver(now, &cap, world);
    }

    /// To the connection's parser, and to the packet log and ring.
    fn deliver(&mut self, now: i64, cap: &TcpChunk, world: &mut dyn World) {
        let dispatcher = self.dispatcher;
        let (assembler, processor) = self.parsers.entry(ports_of(cap)).or_insert_with(|| {
            let mut processor =
                PacketDecoder::new(dispatcher.storage.clone(), dispatcher.skills.clone(), dispatcher.npcs.clone());
            processor.use_tick_skills(dispatcher.dot_ids.clone());
            (Reassembler::new(), processor)
        });
        if assembler.push_bytes(&cap.data, processor) {
            dispatcher.ports.saw_parse(now);
        }
        // After the parser, so the record naming another character opens
        // the packet log's new file.
        // Read only while a packet log is written: this runs for every payload.
        let storage = &dispatcher.storage;
        let you = crate::logging::logger::packet_log_on()
            .then(|| storage.local_identity_from_self_record().then(|| storage.local_character_name()).flatten())
            .flatten();
        world.record(cap, you.as_deref());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::{reply_payload, segment};

    const GAME: &[u8] = &[0x24, 0x04, 0x38, 0x0E, 0x00, 0x36];

    #[derive(Default)]
    struct TestWorld {
        now: i64,
        game: Option<String>,
        looks: Vec<i64>,
        devices: Option<Vec<String>>,
        device_reads: Vec<i64>,
        kept: Vec<TcpChunk>,
    }

    impl World for TestWorld {
        fn game_title(&mut self) -> Option<String> {
            self.looks.push(self.now);
            self.game.clone()
        }
        fn look_alikes(&mut self) -> Vec<String> {
            Vec::new()
        }
        fn device_list(&mut self) -> Option<Vec<String>> {
            self.device_reads.push(self.now);
            self.devices.clone()
        }
        fn record(&mut self, cap: &TcpChunk, _: Option<&str>) {
            self.kept.push(cap.clone());
        }
    }

    /// A dispatcher with empty name tables and its own detector and ping.
    fn dispatcher() -> PacketRouter {
        let ports = Arc::new(PortWatch::new());
        let ping = Arc::new(PingTracker::new());
        let names = (Arc::new(SkillLookup::new()), Arc::new(NpcLookup::new()));
        PacketRouter::new(Arc::new(DataStorage::new()), names.0, names.1, ports, ping)
    }

    /// State started at 0, and a world where the game runs.
    fn start(dispatcher: &PacketRouter) -> (DispatchState<'_>, TestWorld) {
        let world = TestWorld { game: Some("AION2".into()), ..TestWorld::default() };
        (DispatchState::new(dispatcher, 0), world)
    }

    /// A payload that is not the game's, on another connection.
    fn quiet(device: &str, t: i64) -> TcpChunk {
        segment(Some(device), 443, 50000, &[1], t)
    }

    fn give(state: &mut DispatchState, world: &mut TestWorld, now: i64, cap: TcpChunk) {
        world.now = now;
        state.feed(now, cap, world);
    }

    /// Feeds `events` in time order.
    fn give_all(state: &mut DispatchState, world: &mut TestWorld, mut events: Vec<(i64, TcpChunk)>) {
        events.sort_by_key(|(t, _)| *t);
        for (t, cap) in events {
            give(state, world, t, cap);
        }
    }

    /// The game's payloads on `device` from `server` to `client`, every 50 ms
    /// from `from` to `to`, both included.
    fn stream(device: &str, server: u16, client: u16, from: i64, to: i64) -> Vec<(i64, TcpChunk)> {
        (from..=to).step_by(50).map(|t| (t, segment(Some(device), server, client, GAME, t))).collect()
    }

    /// Feeds `events` in time order until a lock; when and what locked.
    fn lock_time(state: &mut DispatchState, world: &mut TestWorld, mut events: Vec<(i64, TcpChunk)>) -> Option<(i64, Option<String>, u16)> {
        events.sort_by_key(|(t, _)| *t);
        for (t, cap) in events {
            give(state, world, t, cap);
            if let Some((device, port)) = state.dispatcher.ports.locked() {
                return Some((t, device, port));
            }
        }
        None
    }

    /// Locks `lo` 13328 to client 50349 with 12 payloads from 0 to 550.
    fn lock_loopback(state: &mut DispatchState, world: &mut TestWorld) {
        let locked = lock_time(state, world, stream("lo", 13328, 50349, 0, 550));
        assert_eq!(locked, Some((550, Some("lo".into()), 13328)));
    }

    #[test]
    fn the_game_is_looked_for_every_10_s_then_every_60_s() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        world.game = None;
        for t in [0, 5_000] {
            give(&mut state, &mut world, t, quiet("lo", t));
        }
        world.game = Some("AION2".into());
        for t in [10_000, 30_000, 69_999, 70_000] {
            give(&mut state, &mut world, t, quiet("lo", t));
        }
        assert_eq!(world.looks, [0, 10_000, 70_000]);
    }

    #[test]
    fn without_the_game_payloads_are_only_counted() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        world.game = None;
        give_all(&mut state, &mut world, stream("lo", 13328, 50349, 0, 950));
        assert_eq!(d.ports.current_port(), None);
        assert!(world.kept.is_empty() && state.rates.is_empty() && state.parsers.is_empty());
        assert_eq!(state.tally.no_game, 20);
        assert_eq!(state.tally.devices.get("lo"), Some(&(20, 20)));
    }

    #[test]
    fn the_game_closing_drops_the_lock_and_all_streams() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        lock_loopback(&mut state, &mut world);
        give(&mut state, &mut world, 1_000, segment(Some("eth0"), 13328, 50001, GAME, 1_000));
        assert!(!state.parsers.is_empty());
        world.game = None;
        give(&mut state, &mut world, 59_999, segment(Some("lo"), 13328, 50349, GAME, 59_999));
        assert_eq!(d.ports.current_port(), Some(13328), "no look yet");
        give(&mut state, &mut world, 60_000, segment(Some("lo"), 13328, 50349, GAME, 60_000));
        assert_eq!(world.looks, [0, 60_000]);
        assert_eq!(d.ports.current_port(), None);
        assert!(state.parsers.is_empty() && state.orders.is_empty() && state.rates.is_empty());
        assert_eq!(state.tally.no_game, 1);
    }

    #[test]
    fn unlocked_payloads_are_counted_per_device() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 0, segment(Some("wlan0"), 443, 50000, &[1, 2, 3], 0));
        give(&mut state, &mut world, 10, segment(Some("wlan0"), 13328, 50349, GAME, 10));
        // No TLS test in the count.
        give(&mut state, &mut world, 20, segment(Some("wlan0"), 443, 50001, &[0x17, 0x03, 0x03, 0x00, 0x10, 0x0E, 0x00, 0x36], 20));
        give(&mut state, &mut world, 30, segment(None, 443, 50000, &[1], 30));
        assert_eq!(state.tally.devices.get("wlan0"), Some(&(3, 2)));
        assert_eq!(state.tally.devices.get("?"), Some(&(1, 0)));
        assert_eq!(state.tally.no_game, 0);
    }

    #[test]
    fn the_unlocked_report_comes_every_30_s_and_not_while_locked() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 1_000, quiet("wlan0", 1_000));
        give(&mut state, &mut world, 29_999, quiet("wlan0", 29_999));
        assert_eq!(state.tally.devices.get("wlan0"), Some(&(2, 0)));
        give(&mut state, &mut world, 30_000, quiet("wlan0", 30_000));
        assert!(state.tally.devices.is_empty(), "reported, counts start over");
        assert_eq!(state.tally_since_ms, 30_000);
        // Locked: no report, and the 30 s start again from each payload.
        let locked = lock_time(&mut state, &mut world, stream("lo", 13328, 50349, 40_000, 40_550));
        assert!(locked.is_some());
        give(&mut state, &mut world, 41_000, segment(Some("lo"), 13328, 50349, GAME, 41_000));
        assert_eq!(state.tally_since_ms, 41_000);
        assert_eq!(state.tally.devices.get("lo"), Some(&(12, 12)), "counted before the lock only");
        d.ports.reset();
        give(&mut state, &mut world, 70_999, quiet("wlan0", 70_999));
        assert_eq!(state.tally.devices.get("wlan0"), Some(&(1, 0)), "not 30 s since the lock's last payload");
        give(&mut state, &mut world, 71_000, quiet("wlan0", 71_000));
        assert!(state.tally.devices.is_empty());
    }

    #[test]
    fn the_unlocked_report_lists_each_device_busiest_first() {
        let mut tally = Tally::default();
        tally.add(&segment(Some("NordLynx Tunnel"), 1, 2, &[0x0E, 0x00, 0x36, 0x01], 0), true);
        tally.add(&segment(Some("NordLynx Tunnel"), 1, 2, &[0x01, 0x02], 0), true);
        tally.add(&segment(Some("Realtek"), 1, 2, &[0x01], 0), true);
        tally.no_game = 3;
        assert_eq!(tally.busiest(), [("NordLynx Tunnel", 2, 1), ("Realtek", 1, 0)]);
        let line = tally.line(30_000, false, &[]);
        assert!(line.contains("30 s") && line.contains("not found") && line.contains("3 payloads ignored"), "{line}");
        assert!(line.contains("aion"), "{line}");
        let nord = line.find("NordLynx Tunnel: 2 payloads, 1 marked").expect(&line);
        let realtek = line.find("Realtek: 1 payloads, 0 marked").expect(&line);
        assert!(nord < realtek);
        let found = tally.line(30_000, true, &[]);
        assert!(found.contains("running") && !found.contains("aion"), "{found}");
        let alike = tally.line(30_000, false, &["comm=\"aion-launcher\"".into()]);
        assert!(alike.contains("aion-launcher"), "{alike}");
    }

    #[test]
    fn the_unlocked_report_names_at_most_8_devices() {
        let mut many = Tally::default();
        for n in 0..10u8 {
            for _ in 0..=n {
                many.add(&segment(Some(&format!("dev{n}")), 1, 2, &[1], 0), true);
            }
        }
        let rows = many.busiest();
        assert_eq!(rows.len(), 8);
        assert_eq!(rows[0].0, "dev9");
        assert!(!rows.iter().any(|r| r.0 == "dev0" || r.0 == "dev1"));
    }

    #[test]
    fn tls_looking_starts() {
        for yes in [&[0x17, 0x03, 0x03][..], &[0x16, 0x03, 0x01], &[0x14, 0x03, 0x00]] {
            assert!(tls_start(yes), "{yes:02x?}");
        }
        for no in [&[0x17, 0x03, 0x05][..], &[0x17, 0x04, 0x03], &[0x13, 0x03, 0x03], &[0x18, 0x03, 0x03], &[0x17, 0x03]] {
            assert!(!tls_start(no), "{no:02x?}");
        }
    }

    #[test]
    fn a_tls_record_header_is_told_from_game_bytes() {
        let records: [&[u8]; 3] = [&[0x17, 0x03, 0x03, 0x05, 0x7a, 0x5a], &[0x16, 0x03, 0x03, 0x00, 0x5d], &[0x17, 0x03, 0x03, 0x41, 0x00]];
        for data in records {
            assert!(is_tls_record(data), "{data:02x?}");
        }
        let others: [&[u8]; 6] = [
            &[0x34, 0x21, 0x36, 0x01, 0x00, 0x00],
            &[0x24, 0x04, 0x38, 0xe8, 0x81, 0x02],
            &[0x17, 0x03, 0x03, 0x05],
            &[0x17, 0x03, 0x03, 0xff, 0xff],
            &[0x17, 0x03, 0x03, 0x41, 0x01],
            &[0x17, 0x03, 0x03, 0x00, 0x00],
        ];
        for data in others {
            assert!(!is_tls_record(data), "{data:02x?}");
        }
    }

    #[test]
    fn a_tls_connection_from_the_game_port_is_left_out_whole() {
        let mut filter = ConnectionFilter::default();
        let from_game = |to: u16, data: &[u8]| segment(Some("eth0"), 13328, to, data, 0);
        assert!(filter.admit(&from_game(50349, &[0x24, 0x04, 0x38, 0xe8, 0x81, 0x02])));
        assert!(!filter.admit(&from_game(50350, &[0x17, 0x03, 0x03, 0x05, 0x7a, 0x5a, 0x5a])));
        assert!(filter.admit(&from_game(50349, &[0x34, 0x21, 0x36, 0x01])));
        assert!(!filter.admit(&from_game(50350, &[0x5a, 0x5a, 0x04, 0x38])), "the whole connection");
        assert!(filter.admit(&from_game(50349, &[0x17, 0x03, 0x03, 0x00, 0x10])), "judged already");
        let mut other_server = from_game(50350, &[0x24, 0x04, 0x38]);
        other_server.src_ip = Some("193.202.112.99".into());
        assert!(filter.admit(&other_server), "another server, another connection");
        filter.clear();
        assert!(filter.admit(&from_game(50350, &[0x24, 0x04, 0x38])));
    }

    #[test]
    fn only_marked_plain_payloads_count_before_a_lock() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        let tls_marked = [0x17, 0x03, 0x03, 0x00, 0x10, 0x01, 0x0E, 0x00, 0x36];
        give(&mut state, &mut world, 0, segment(Some("lo"), 13328, 50001, &tls_marked, 0));
        give(&mut state, &mut world, 10, segment(Some("lo"), 13328, 50002, &[0x01, 0x02, 0x03], 10));
        give(&mut state, &mut world, 20, segment(Some("lo"), 13328, 50003, GAME, 20));
        assert!(!state.rates.contains_key(&(13328, 50001)) && !state.rates.contains_key(&(13328, 50002)));
        assert_eq!(state.rates.get(&(13328, 50003)), Some(&(1, 20)));
        assert_eq!(world.kept.len(), 1);
        // 50001 was judged TLS on its first payload: its game bytes stay out.
        give(&mut state, &mut world, 30, segment(Some("lo"), 13328, 50001, GAME, 30));
        assert!(!state.rates.contains_key(&(13328, 50001)));
        // 50002's first payload was plain: its game bytes count.
        give(&mut state, &mut world, 40, segment(Some("lo"), 13328, 50002, GAME, 40));
        assert_eq!(state.rates.get(&(13328, 50002)), Some(&(1, 40)));
        // The client's side judges the connection too.
        give(&mut state, &mut world, 50, segment(Some("lo"), 50004, 13328, &[0x16, 0x03, 0x01, 0x00, 0x20], 50));
        give(&mut state, &mut world, 60, segment(Some("lo"), 13328, 50004, GAME, 60));
        assert!(!state.rates.contains_key(&(13328, 50004)));
    }

    #[test]
    fn twelve_payloads_50_ms_apart_count_12_at_550() {
        // On a device that must wait, so no lock ends the count.
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give_all(&mut state, &mut world, stream("eth0", 13328, 50349, 0, 550));
        assert_eq!(state.rates.get(&(13328, 50349)), Some(&(12, 550)));
    }

    /// 11 payloads 50 apart from 0, then one at `last`; the count after it.
    /// A pick that is there holds eth0 off for 12 s, so nothing locks.
    fn count_after_gap(last: i64) -> Option<u32> {
        let d = dispatcher();
        d.ports.set_preferred_device(Some("vpn0".into()));
        let (mut state, mut world) = start(&d);
        let mut events = stream("eth0", 13328, 50349, 0, 500);
        events.push((last, segment(Some("eth0"), 13328, 50349, GAME, last)));
        give_all(&mut state, &mut world, events);
        state.rates.get(&(13328, 50349)).map(|rate| rate.0)
    }

    #[test]
    fn a_gap_over_3000_ms_starts_the_count_again() {
        assert_eq!(count_after_gap(3_501), Some(1));
    }

    #[test]
    fn a_gap_of_exactly_3000_ms_keeps_the_count() {
        assert_eq!(count_after_gap(3_500), Some(12));
    }

    #[test]
    fn one_stray_marked_payload_counts_once_and_does_not_lock() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 0, segment(Some("lo"), 57343, 33072, &[0x06, 0x00, 0x36], 0));
        assert_eq!(state.rates.get(&(33072, 57343)), Some(&(1, 0)));
        assert_eq!(d.ports.current_port(), None);
    }

    #[test]
    fn a_counted_payload_is_parsed_kept_and_offered_before_the_lock() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 0, segment(Some("wlan0"), 13328, 50349, GAME, 7));
        assert_eq!(world.kept.len(), 1);
        assert_eq!(world.kept[0].captured_at_ms, 7);
        assert!(state.parsers.contains_key(&(13328, 50349)));
        // Offered on its device: locking it later with no device keeps wlan0.
        assert!(d.ports.try_lock(13328, None, 2_500));
        assert_eq!(d.ports.current_device().as_deref(), Some("wlan0"));
    }

    #[test]
    fn the_lock_takes_the_server_port_and_device() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        let locked = lock_time(&mut state, &mut world, stream("lo", 40404, 50349, 0, 1_000));
        assert_eq!(locked, Some((550, Some("lo".into()), 40404)));
    }

    #[test]
    fn a_physical_device_locks_at_2500_ms() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        let locked = lock_time(&mut state, &mut world, stream("eth0", 13328, 50349, 0, 5_000));
        assert_eq!(locked, Some((2_500, Some("eth0".into()), 13328)), "its 51st payload");
    }

    #[test]
    fn the_wait_runs_from_any_connections_first_offer() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        let mut events = vec![(0, segment(Some("eth0"), 40000, 50300, GAME, 0))];
        events.extend(stream("eth0", 13328, 50349, 2_000, 6_000));
        assert_eq!(lock_time(&mut state, &mut world, events), Some((2_550, Some("eth0".into()), 13328)), "its 12th");
    }

    #[test]
    fn a_ping_reducer_locks_the_loopback_copy() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        let mut events = stream("eth0", 13328, 50349, 0, 5_000);
        events.extend(stream("lo", 40000, 50500, 1_000, 5_000));
        assert_eq!(lock_time(&mut state, &mut world, events), Some((1_550, Some("lo".into()), 40000)));
    }

    #[test]
    fn a_picked_device_holds_off_others_for_12_s() {
        let d = dispatcher();
        d.ports.set_preferred_device(Some("vpn0".into()));
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 0, quiet("wlan0", 0));
        assert!(state.follow.may_lock(1_000, Some("vpn0"), false));
        assert!(!state.follow.may_lock(1_000, Some("eth0"), false));
        assert!(state.follow.may_lock(12_000, Some("eth0"), false));
        assert!(state.follow.may_lock(1_000, Some("eth0"), true), "the pick is absent");
        // A counted payload on the pick at 12,000.
        give(&mut state, &mut world, 12_000, segment(Some("VPN0"), 13328, 50349, GAME, 12_000));
        assert!(!state.follow.may_lock(23_999, Some("eth0"), false));
        assert!(state.follow.may_lock(24_000, Some("eth0"), false));
    }

    #[test]
    fn the_head_start_runs_from_the_first_payload_past_the_gate() {
        let d = dispatcher();
        d.ports.set_preferred_device(Some("vpn0".into()));
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 5_000, quiet("wlan0", 5_000));
        assert!(!state.follow.may_lock(16_999, Some("eth0"), false));
        assert!(state.follow.may_lock(17_000, Some("eth0"), false));
    }

    /// When eth0 locks with the pick vpn0 and the device list `list`.
    fn lock_beside_pick(list: &[&str]) -> Option<i64> {
        let d = dispatcher();
        d.ports.set_preferred_device(Some("vpn0".into()));
        let (mut state, mut world) = start(&d);
        world.devices = Some(list.iter().map(|name| name.to_string()).collect());
        lock_time(&mut state, &mut world, stream("eth0", 13328, 50349, 0, 20_000)).map(|lock| lock.0)
    }

    #[test]
    fn a_listed_pick_holds_another_device_off_until_12_s() {
        assert_eq!(lock_beside_pick(&["lo", "eth0", "vpn0"]), Some(12_000));
    }

    #[test]
    fn a_pick_missing_from_the_list_holds_nothing_off() {
        assert_eq!(lock_beside_pick(&["lo", "eth0"]), Some(2_500));
    }

    #[test]
    fn the_pick_counts_as_absent_only_when_listed_without_it() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        world.devices = Some(vec!["lo".into(), "eth0".into()]);
        assert!(state.pick_absent(0, Some("vpn0"), &mut world));
        world.devices = Some(vec!["lo".into(), "VPN0".into()]);
        assert!(!state.pick_absent(1_000, Some("vpn0"), &mut world));
        world.devices = Some(Vec::new());
        assert!(state.pick_absent(2_000, Some("vpn0"), &mut world));
        world.devices = None;
        assert!(!state.pick_absent(3_000, Some("vpn0"), &mut world), "not listed yet");
        assert!(!state.pick_absent(4_000, None, &mut world));
    }

    #[test]
    fn the_device_list_is_read_at_most_once_a_second_and_only_with_a_pick() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 0, quiet("wlan0", 0));
        assert!(world.device_reads.is_empty(), "no pick, no list");
        d.ports.set_preferred_device(Some("vpn0".into()));
        for t in [100, 600, 1_099, 1_100, 1_500, 2_200] {
            give(&mut state, &mut world, t, quiet("wlan0", t));
        }
        assert_eq!(world.device_reads, [100, 1_100, 2_200]);
    }

    #[test]
    fn a_lock_keeps_only_its_own_parser_and_clears_counts() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        let mut events = stream("eth0", 13328, 50001, 0, 400);
        events.extend(stream("lo", 13328, 50349, 0, 550));
        lock_time(&mut state, &mut world, events);
        assert_eq!(d.ports.locked(), Some((Some("lo".into()), 13328)));
        assert_eq!(state.parsers.keys().copied().collect::<Vec<_>>(), [(13328, 50349)]);
        assert!(state.rates.is_empty());
    }

    #[test]
    fn the_locked_device_is_compared_trimmed_and_case_blind() {
        assert!(on_locked_device(Some("eth0"), Some(" ETH0 ")));
        assert!(on_locked_device(Some("eth0"), Some("eth0")));
        assert!(!on_locked_device(Some("eth0"), None));
        assert!(!on_locked_device(Some("eth0"), Some("")));
        assert!(!on_locked_device(Some("eth0"), Some("wlan0")));
        assert!(on_locked_device(None, None) && on_locked_device(None, Some("wlan0")));
    }

    #[test]
    fn the_clients_packets_keep_the_lock_alive() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        assert!(lock_time(&mut state, &mut world, stream("wlan0", 13328, 50349, 0, 2_500)).is_some());
        let mut events: Vec<(i64, TcpChunk)> =
            (2_550..=30_000).step_by(50).map(|t| (t, segment(Some("wlan0"), 50349, 13328, &[0x01], t))).collect();
        events.extend(stream("eth0", 13328, 50001, 3_000, 30_000));
        give_all(&mut state, &mut world, events);
        assert_eq!(d.ports.locked(), Some((Some("wlan0".into()), 13328)));
        assert!(world.kept.iter().all(|k| k.device_name.as_deref() == Some("wlan0") && k.src_port == 13328));
    }

    #[test]
    fn a_ping_reading_counts_as_a_parse_even_on_a_repeat() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        lock_loopback(&mut state, &mut world);
        let arrival = 1_790_000_000_000;
        let mut first = segment(Some("lo"), 13328, 50349, &[0x01; 16], arrival);
        first.tcp_seq = Some(5_000);
        give(&mut state, &mut world, 1_000, first);
        let kept = world.kept.len();
        // A reply echoing the client's clock 80 ms back, in bytes the parser
        // already had: the TCP order drops it, the ping tracker reads it.
        let mut repeat = segment(Some("lo"), 13328, 50349, &reply_payload(arrival - 80), arrival);
        repeat.tcp_seq = Some(5_000);
        give(&mut state, &mut world, 9_000, repeat);
        assert_eq!(world.kept.len(), kept, "a repeat");
        assert_eq!(d.ping.current_ping_ms(), Some(80));
        assert_eq!(d.ports.last_parse(), Some(9_000));
    }

    #[test]
    fn only_server_bytes_reach_the_parser_each_once() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        lock_loopback(&mut state, &mut world);
        let before = world.kept.len();
        let with_seq = |from: u16, to: u16, seq: u32, data: &[u8], t: i64| {
            let mut cap = segment(Some("lo"), from, to, data, t);
            cap.tcp_seq = Some(seq);
            cap
        };
        give(&mut state, &mut world, 1_000, with_seq(13328, 50349, 100, &[1, 2, 3, 4], 1));
        give(&mut state, &mut world, 1_010, with_seq(13328, 50349, 100, &[1, 2, 3, 4], 2));
        give(&mut state, &mut world, 1_020, with_seq(50349, 13328, 900, &[9, 9], 3));
        give(&mut state, &mut world, 1_030, with_seq(13328, 50349, 102, &[3, 4, 5, 6], 4));
        // Out of order: held until the gap fills, then both in order.
        give(&mut state, &mut world, 1_040, with_seq(13328, 50349, 108, &[9], 5));
        give(&mut state, &mut world, 1_050, with_seq(13328, 50349, 106, &[7, 8], 6));
        // A second connection from the game port that talks TLS.
        give(&mut state, &mut world, 1_060, segment(Some("lo"), 13328, 50350, &[0x17, 0x03, 0x03, 0x00, 0x08], 7));
        give(&mut state, &mut world, 1_070, segment(Some("lo"), 13328, 50350, GAME, 8));
        // A payload with no sequence number goes as it is.
        give(&mut state, &mut world, 1_080, segment(Some("lo"), 13328, 50351, &[0x0A], 9));
        let got: Vec<(i64, Vec<u8>)> = world.kept[before..].iter().map(|k| (k.captured_at_ms, k.data.clone())).collect();
        assert_eq!(got, [(1, vec![1, 2, 3, 4]), (4, vec![5, 6]), (6, vec![7, 8, 9]), (9, vec![0x0A])]);
    }

    #[test]
    fn a_lock_with_no_parse_for_120_s_is_let_go() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        lock_loopback(&mut state, &mut world);
        d.ports.saw_parse(1_000);
        give(&mut state, &mut world, 121_000, quiet("lo", 121_000));
        assert_eq!(d.ports.current_port(), Some(13328));
        give(&mut state, &mut world, 121_001, quiet("lo", 121_001));
        assert_eq!(d.ports.current_port(), None);
        assert!(state.parsers.is_empty() && state.rates.is_empty());
    }

    #[test]
    fn no_report_comes_at_once_when_a_silent_lock_is_let_go() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        d.ports.move_to(13328, Some("eth0"));
        give(&mut state, &mut world, 2_000, segment(Some("eth0"), 50349, 13328, &[1], 2_000));
        d.ports.saw_parse(1_000);
        give(&mut state, &mut world, 130_000, quiet("eth0", 130_000));
        assert_eq!(d.ports.current_port(), None);
        assert!(state.tally.devices.is_empty(), "the payload that let go is not counted");
        assert_eq!(state.tally_since_ms, 130_000);
        give(&mut state, &mut world, 159_999, quiet("eth0", 159_999));
        assert_eq!(state.tally.devices.get("eth0"), Some(&(1, 0)), "no report yet");
        give(&mut state, &mut world, 160_000, quiet("eth0", 160_000));
        assert!(state.tally.devices.is_empty(), "reported");
    }

    #[test]
    fn a_lock_never_parsed_is_never_let_go() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        d.ports.offer(13328, Some("lo"), 0);
        assert!(d.ports.try_lock(13328, Some("lo"), 0));
        give(&mut state, &mut world, 500_000, quiet("lo", 500_000));
        assert_eq!(d.ports.current_port(), Some(13328));
    }

    #[test]
    fn each_connection_gets_its_own_parser() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give(&mut state, &mut world, 0, segment(Some("eth0"), 13328, 50001, GAME, 0));
        give(&mut state, &mut world, 10, segment(Some("eth0"), 13328, 50002, GAME, 10));
        give(&mut state, &mut world, 20, segment(Some("eth0"), 13328, 50001, GAME, 20));
        let mut keys: Vec<(u16, u16)> = state.parsers.keys().copied().collect();
        keys.sort();
        assert_eq!(keys, [(13328, 50001), (13328, 50002)]);
    }

    #[test]
    fn a_moved_lock_judges_connections_again() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        lock_loopback(&mut state, &mut world);
        give(&mut state, &mut world, 600, segment(Some("lo"), 13328, 50350, &[0x17, 0x03, 0x03, 0x00, 0x08], 0));
        assert_eq!(state.filter.tls.len(), 1);
        d.ports.move_to(13329, Some("lo"));
        give(&mut state, &mut world, 700, quiet("lo", 0));
        assert!(state.filter.tls.is_empty() && state.filter.seen.is_empty());
    }

    #[test]
    fn a_reset_from_the_capture_page_keeps_the_parsers() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        lock_loopback(&mut state, &mut world);
        let mut cap = segment(Some("lo"), 13328, 50349, &[1, 2], 0);
        cap.tcp_seq = Some(10);
        give(&mut state, &mut world, 600, cap);
        assert_eq!(state.orders.len(), 1);
        d.ports.reset();
        give(&mut state, &mut world, 700, quiet("lo", 0));
        assert!(state.orders.is_empty() && state.filter.seen.len() == 1, "only the new payload judged");
        assert_eq!(state.parsers.keys().copied().collect::<Vec<_>>(), [(13328, 50349)]);
        give(&mut state, &mut world, 800, segment(Some("lo"), 13328, 50349, GAME, 0));
        assert_eq!(state.rates.get(&(13328, 50349)), Some(&(1, 800)));
    }

    #[test]
    fn the_game_is_followed_and_its_kept_payloads_go_first() {
        let d = dispatcher();
        let (mut state, mut world) = start(&d);
        give_all(&mut state, &mut world, stream("wlan0", 13328, 50349, 0, 10_000));
        assert_eq!(d.ports.locked(), Some((Some("wlan0".into()), 13328)));
        let mut cable = stream("eth0", 13328, 50001, 10_000, 30_000);
        let after_22 = cable.split_off(cable.iter().position(|(t, _)| *t > 22_000).unwrap());
        for (t, cap) in cable {
            if t == 22_000 {
                assert_eq!(d.ports.current_device().as_deref(), Some("wlan0"), "not before 22,000");
            }
            give(&mut state, &mut world, t, cap);
        }
        assert_eq!(d.ports.locked(), Some((Some("eth0".into()), 13328)));
        let moved: Vec<i64> = world.kept.iter().filter(|k| k.device_name.as_deref() == Some("eth0")).map(|k| k.captured_at_ms).collect();
        assert_eq!(moved.len(), 241);
        assert_eq!(moved, (10_000..=22_000).step_by(50).collect::<Vec<i64>>(), "oldest first");
        give_all(&mut state, &mut world, after_22.into_iter().take(3).collect());
        assert_eq!(world.kept.last().map(|k| k.captured_at_ms), Some(22_150));
    }
}
