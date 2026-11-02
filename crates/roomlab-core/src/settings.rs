//! Data-directory resolution (§16.2) and persisted settings (no credentials, ever).

use std::fs;
use std::path::{Path, PathBuf};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::contracts::ProviderConfig;
use crate::error::Result;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DataPaths {
    pub data: PathBuf,
    pub db_file: PathBuf,
    pub settings_file: PathBuf,
    pub agents: PathBuf,
    pub rooms: PathBuf,
    pub knowledge: PathBuf,
    pub attachments: PathBuf,
    pub runs: PathBuf,
    pub exports: PathBuf,
    pub logs: PathBuf,
    /// Set when the portable folder was not writable and a fallback was chosen.
    pub notice: Option<String>,
    pub portable: bool,
}

impl DataPaths {
    pub fn at(root: &Path, portable: bool, notice: Option<String>) -> Result<Self> {
        let data = root.join("data");
        let p = DataPaths {
            db_file: data.join("roomlab.sqlite"),
            settings_file: data.join("settings.json"),
            agents: data.join("agents"),
            rooms: data.join("rooms"),
            knowledge: data.join("knowledge"),
            attachments: data.join("attachments"),
            runs: data.join("runs"),
            exports: root.join("exports"),
            logs: root.join("logs"),
            data,
            notice,
            portable,
        };
        for d in [&p.data, &p.agents, &p.rooms, &p.knowledge, &p.attachments, &p.runs, &p.exports, &p.logs] {
            fs::create_dir_all(d)?;
        }
        Ok(p)
    }

    /// Resolve the data root: explicit override → portable folder next to the exe (if writable)
    /// → per-user %LOCALAPPDATA%\RoomLab with a visible notice.
    pub fn resolve(exe_dir: &Path, override_dir: Option<PathBuf>) -> Result<Self> {
        if let Some(o) = override_dir {
            return Self::at(&o, false, Some(format!("Using data directory override: {}", o.display())));
        }
        if is_writable(exe_dir) {
            return Self::at(exe_dir, true, None);
        }
        let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
        let root = base.join("RoomLab");
        fs::create_dir_all(&root)?;
        Self::at(
            &root,
            false,
            Some(format!(
                "The RoomLab folder ({}) is not writable, so data is stored in {}. Use Observatory → Export to move runs.",
                exe_dir.display(),
                root.display()
            )),
        )
    }
}

pub fn is_writable(dir: &Path) -> bool {
    let probe = dir.join(format!(".roomlab_write_probe_{}", std::process::id()));
    match fs::write(&probe, b"probe") {
        Ok(_) => {
            let _ = fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct Settings {
    pub first_run_complete: bool,
    /// DEMO | LOCAL_CONNECTED
    pub mode: String,
    /// SIMPLE | ADVANCED (presentation only; never changes privileges)
    pub settings_mode: String,
    pub active_run_id: Option<String>,
    pub provider: ProviderConfig,
    pub demo_turn_delay_ms: u64,
    pub reduced_motion: String,
    /// Default spawn mode for new rooms: OFF | ASK | AUTO_WITH_CAP
    pub spawn_mode: String,
    pub max_children: i64,
    pub soma_default: String,
    pub pause_cancels_inflight: bool,
    pub managed_llama_exe: Option<String>,
    pub managed_model_path: Option<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            first_run_complete: false,
            mode: "DEMO".into(),
            settings_mode: "SIMPLE".into(),
            active_run_id: None,
            provider: ProviderConfig::default(),
            demo_turn_delay_ms: 650,
            reduced_motion: "FOLLOW_OS".into(),
            spawn_mode: "ASK".into(),
            max_children: 3,
            soma_default: "LIGHT".into(),
            pause_cancels_inflight: false,
            managed_llama_exe: None,
            managed_model_path: None,
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Settings {
        match fs::read_to_string(path) {
            Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
            Err(_) => Settings::default(),
        }
    }
    pub fn save(&self, path: &Path) -> Result<()> {
        // ProviderConfig.api_key is #[serde(skip)], so secrets never reach disk.
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        fs::rename(&tmp, path)?;
        Ok(())
    }
}
