//! The live meter without a window, for the Qt meter (src-qml): capture
//! (through the capture helper where the OS has one), the dispatcher with
//! its game-port detection, and the meter, wired together. Saves nothing
//! and reads no settings.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::capture::tcp_chunk::TcpChunk;
use crate::capture::port_watch::PortWatch;
use crate::capture::dispatcher::PacketRouter;
use crate::capture::ping_tracker::PingTracker;
use crate::combat::data_storage::DataStorage;
use crate::combat::dps_calculator::DpsCalculator;
use crate::i18n::lookup::{load_language, NpcLookup, SkillLookup};
use crate::platform;

/// The skills whose damage comes over time, from the game data folder.
pub fn dot_skill_ids(data: &Path) -> HashSet<i32> {
    std::fs::read_to_string(data.join("dot_skill_ids.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Vec<i32>>(&text).ok())
        .unwrap_or_default()
        .into_iter()
        .collect()
}

pub struct LiveFeed {
    pub storage: Arc<DataStorage>,
    pub meter: Mutex<DpsCalculator>,
    pub ping: Arc<PingTracker>,
    pub ports: Arc<PortWatch>,
    /// The game's skill and monster names, and the folder they load from.
    skills: Arc<SkillLookup>,
    npcs: Arc<NpcLookup>,
    data: Option<PathBuf>,
}

impl LiveFeed {
    /// Starts capturing. `data` is the game data folder (skill and NPC
    /// names, the boss and dummy tables) and `language` the language of the
    /// names. `helper` and `notify` are as in `live::start_with_helper`.
    /// `device` is the capture device the player picked, set before the
    /// capture starts.
    pub fn start(
        data: Option<&Path>,
        language: &str,
        helper: Option<PathBuf>,
        device: Option<String>,
        notify: impl Fn(bool) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let skills = SkillLookup::new();
        let npcs = NpcLookup::new();
        let mut dot_ids = HashSet::new();
        match data {
            Some(data) => {
                dot_ids = dot_skill_ids(data);
                load_language(&skills, &npcs, data, language);
            }
            None => warn!("No game data folder: skills and monsters go by number"),
        }
        let skills = Arc::new(skills);
        let npcs = Arc::new(npcs);

        let storage = Arc::new(DataStorage::new());
        if let Some(data) = data {
            storage.set_abnormal_stack_limits(crate::capture::abnormal::stack_limits(data));
        }
        let ping = Arc::new(PingTracker::using_clock(platform::clock::perf_clock()));
        let ports = Arc::new(PortWatch::new());
        ports.set_preferred_device(device);
        let meter = DpsCalculator::new(storage.clone(), skills.clone(), npcs.clone(), ping.clone());

        let (sender, receiver) = mpsc::channel::<TcpChunk>(4096);
        if platform::pcap::library_available() {
            crate::capture::live::start_with_helper(helper, sender, notify);
        } else {
            error!("The capture library did not load: packet capture is off. {}", platform::pcap::MISSING_HELP);
            notify(false);
        }

        let names = (skills.clone(), npcs.clone());
        let mut dispatcher = PacketRouter::new(storage.clone(), skills, npcs, ports.clone(), ping.clone());
        dispatcher.use_tick_skills(dot_ids);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("no runtime for the capture: {e}"))?;
        std::thread::Builder::new()
            .name("capture-dispatch".into())
            .spawn(move || {
                runtime.block_on(dispatcher.run(receiver));
                info!("Capture dispatch ended");
            })
            .map_err(|e| format!("no thread for the capture: {e}"))?;

        Ok(Self {
            storage,
            meter: Mutex::new(meter),
            ping,
            ports,
            skills: names.0,
            npcs: names.1,
            data: data.map(Path::to_path_buf),
        })
    }

    /// Loads the skill and monster names in `language`, as 1.x does when its
    /// language setting changes (`set_language`): names read from then on
    /// use it.
    pub fn set_language(&self, language: &str) {
        if let Some(data) = &self.data {
            load_language(&self.skills, &self.npcs, data, language);
        }
    }

    /// Stops the capture helper: the program is quitting.
    pub fn stop(&self) {
        crate::capture::live::stop();
    }
}
