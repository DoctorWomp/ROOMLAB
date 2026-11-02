//! RoomLab trusted control plane.
//!
//! The Rust core is the only production authority for room access, identities, world facts,
//! action effects, token budgets and spawn slots (§2.2). The GUI sends operator intents;
//! model output is treated as untrusted data and passes through `actions::apply`.

pub mod actions;
pub mod agent;
pub mod attachments;
pub mod auth;
pub mod comms;
pub mod context;
pub mod contracts;
pub mod crucible;
pub mod db;
pub mod demo;
pub mod error;
pub mod events;
pub mod experiments;
pub mod ids;
pub mod knowledge;
pub mod narrative;
pub mod ops;
pub mod org;
pub mod providers;
pub mod redact;
pub mod room;
pub mod runner;
pub mod scenarios;
pub mod scheduler;
pub mod settings;
pub mod soma;
pub mod spawn;
pub mod tools;
pub mod world;

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, RwLock};

use rusqlite::{params, Connection};

pub use error::{CoreError, Result};
use settings::{DataPaths, Settings};

pub const HUMAN_ID: &str = "human_01";
pub const WORKSPACE_ID: &str = "ws_soma_lab";

/// Backend-enforced session view role (§12.6). The frontend cannot pass a role label;
/// truth-revealing commands check this value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ViewRole {
    Operator,
    Participant,
}

pub struct Engine {
    conn: Mutex<Connection>,
    pub paths: DataPaths,
    settings: RwLock<Settings>,
    view_role: RwLock<ViewRole>,
    /// Provider secrets live in memory only (never in settings.json, DB, exports or prompts).
    secrets: Mutex<HashMap<String, String>>,
    /// Set to request cancellation of the in-flight generation.
    pub cancel_flag: AtomicBool,
    /// Owned llama-server sidecar (MANAGED_LOCAL), if started by this app.
    pub managed: tokio::sync::Mutex<Option<providers::managed_llama::ManagedLlama>>,
}

impl Engine {
    pub fn open(paths: DataPaths) -> Result<Engine> {
        let conn = db::open(&paths.db_file)?;
        let settings = Settings::load(&paths.settings_file);
        let e = Engine {
            conn: Mutex::new(conn),
            paths,
            settings: RwLock::new(settings),
            view_role: RwLock::new(ViewRole::Operator),
            secrets: Mutex::new(HashMap::new()),
            cancel_flag: AtomicBool::new(false),
            managed: tokio::sync::Mutex::new(None),
        };
        e.bootstrap()?;
        Ok(e)
    }

    /// Fresh engine in a temporary directory (tests and benchmarks).
    pub fn open_temp(dir: &std::path::Path) -> Result<Engine> {
        let paths = DataPaths::at(dir, false, None)?;
        Engine::open(paths)
    }

    fn bootstrap(&self) -> Result<()> {
        self.tx(|c| {
            c.execute(
                "INSERT OR IGNORE INTO workspaces(id, name, created_at) VALUES (?1, 'SOMA Lab', ?2)",
                params![WORKSPACE_ID, ids::now_utc()],
            )?;
            for (id, kind, desc) in [
                ("demo", "DEMO", "Deterministic scripted (L0) policies. No model."),
                ("local/default", "LOCAL_CONNECTED", "Model selected in Settings on the connected local endpoint."),
                ("room_default_subagent", "LOCAL_CONNECTED", "Sub-agent route: inherits the parent's resolved route."),
            ] {
                c.execute(
                    "INSERT OR IGNORE INTO model_routes(id, provider_kind, model_id, enabled, description) VALUES (?1, ?2, NULL, 1, ?3)",
                    params![id, kind, desc],
                )?;
            }
            // Crash recovery (T10): any job left RUNNING by a forced stop is marked FAILED, never re-run silently.
            c.execute(
                "UPDATE jobs SET status='FAILED', outcome='INTERRUPTED_BY_RESTART', finished_wall_ms=?1 WHERE status='RUNNING'",
                params![ids::now_ms()],
            )?;
            spawn::recover_after_crash(c)?;
            Ok(())
        })
    }

    pub fn conn(&self) -> MutexGuard<'_, Connection> {
        self.conn.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Run `f` inside one transaction. Any error rolls back every mutation (T60).
    pub fn tx<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let mut guard = self.conn();
        let tx = guard.transaction()?;
        let out = f(&tx)?;
        tx.commit()?;
        Ok(out)
    }

    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        let guard = self.conn();
        f(&guard)
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap_or_else(|e| e.into_inner()).clone()
    }

    pub fn update_settings(&self, f: impl FnOnce(&mut Settings)) -> Result<Settings> {
        let mut s = self.settings.write().unwrap_or_else(|e| e.into_inner());
        f(&mut s);
        s.save(&self.paths.settings_file)?;
        Ok(s.clone())
    }

    pub fn view_role(&self) -> ViewRole {
        *self.view_role.read().unwrap_or_else(|e| e.into_inner())
    }

    pub(crate) fn set_view_role_internal(&self, role: ViewRole) {
        *self.view_role.write().unwrap_or_else(|e| e.into_inner()) = role;
    }

    pub fn require_operator(&self) -> Result<()> {
        if self.view_role() == ViewRole::Operator {
            Ok(())
        } else {
            Err(CoreError::Forbidden(
                "Operator view required. Participant view cannot open omniscient inspection.".into(),
            ))
        }
    }

    pub fn set_secret(&self, key: &str, value: Option<String>) {
        let mut s = self.secrets.lock().unwrap_or_else(|e| e.into_inner());
        match value {
            Some(v) if !v.is_empty() => {
                s.insert(key.to_string(), v);
            }
            _ => {
                s.remove(key);
            }
        }
    }

    pub fn secret(&self, key: &str) -> Option<String> {
        self.secrets.lock().unwrap_or_else(|e| e.into_inner()).get(key).cloned()
    }

    /// All secret values (for redaction of logs/exports).
    pub fn secret_values(&self) -> Vec<String> {
        self.secrets.lock().unwrap_or_else(|e| e.into_inner()).values().cloned().collect()
    }

    pub fn request_cancel(&self) {
        self.cancel_flag.store(true, Ordering::SeqCst);
    }

    pub fn active_run(&self) -> Result<String> {
        self.settings()
            .active_run_id
            .ok_or_else(|| CoreError::NotFound("no active run; complete first-run setup".into()))
    }
}
