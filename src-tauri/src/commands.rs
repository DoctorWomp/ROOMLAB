//! Explicit, allowlisted Tauri command API (§12.4). Every command validates its inputs and
//! delegates to the trusted core. Truth-revealing commands check the *backend* view role.
//! Tauri capabilities alone do not sandbox Rust code; this module is the boundary.

use std::sync::Arc;

use base64::Engine as _;
use roomlab_core::auth::Principal;
use roomlab_core::contracts::*;
use roomlab_core::experiments::{self, DirectorChange, ExportKind};
use roomlab_core::providers::{self, managed_llama, openai_compat};
use roomlab_core::room::RoomSettings;
use roomlab_core::{agent, attachments, crucible, events, knowledge, org, room, scenarios, scheduler, spawn, CoreError, Engine, ViewRole, HUMAN_ID};
use serde::Deserialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, State};

use crate::AppState;

type R<T> = Result<T, String>;

fn e2s(e: CoreError) -> String {
    match e {
        CoreError::Rejected { code, message } => format!("{}: {message}", code.as_str()),
        other => other.to_string(),
    }
}

fn eng(s: &State<'_, AppState>) -> Arc<Engine> {
    s.engine.clone()
}

fn changed(app: &AppHandle, s: &State<'_, AppState>, run: &str, reason: &str) {
    s.wake.notify_one();
    let _ = app.emit("roomlab://changed", json!({"run_id": run, "reason": reason}));
}

fn short(s: &str, max: usize, what: &str) -> R<()> {
    if s.chars().count() > max {
        return Err(format!("{what} is too long (max {max} characters)"));
    }
    Ok(())
}

fn ident(s: &str) -> R<()> {
    if s.is_empty() || s.len() > 64 || !s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':')) {
        return Err("invalid identifier".into());
    }
    Ok(())
}

fn viewer(e: &Engine) -> Option<&'static str> {
    match e.view_role() {
        ViewRole::Operator => None,
        ViewRole::Participant => Some(HUMAN_ID),
    }
}

fn is_controlled(e: &Engine, run: &str) -> bool {
    e.read(|c| experiments::session_type(c, run)).map(|s| s == SessionType::ControlledTrial).unwrap_or(false)
}

// ───────────────────────────── App / first run / settings ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn bootstrap(state: State<'_, AppState>) -> R<Value> {
    let e = eng(&state);
    let s = e.settings();
    Ok(json!({
        "app_version": APP_VERSION,
        "schema_version": SCHEMA_VERSION,
        "settings": s,
        "data_dir": e.paths.data.display().to_string(),
        "exports_dir": e.paths.exports.display().to_string(),
        "notice": e.paths.notice,
        "portable": e.paths.portable,
        "view_role": e.view_role(),
        "webview_version": tauri::webview_version().ok(),
        "secret_set": e.secret("provider").is_some(),
    }))
}

#[tauri::command(rename_all = "snake_case")]
fn complete_first_run(app: AppHandle, state: State<'_, AppState>, mode: String) -> R<String> {
    let e = eng(&state);
    let run = e.first_run(&mode).map_err(e2s)?;
    changed(&app, &state, &run, "first_run");
    Ok(run)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SettingsPatch {
    settings_mode: Option<String>,
    demo_turn_delay_ms: Option<u64>,
    spawn_mode: Option<String>,
    max_children: Option<i64>,
    soma_default: Option<String>,
    reduced_motion: Option<String>,
    pause_cancels_inflight: Option<bool>,
}

#[tauri::command(rename_all = "snake_case")]
fn save_settings(app: AppHandle, state: State<'_, AppState>, patch: SettingsPatch) -> R<Value> {
    let e = eng(&state);
    if let Some(m) = &patch.settings_mode {
        if !matches!(m.as_str(), "SIMPLE" | "ADVANCED") {
            return Err("settings_mode must be SIMPLE or ADVANCED".into());
        }
    }
    if let Some(m) = &patch.spawn_mode {
        if !matches!(m.as_str(), "OFF" | "ASK" | "AUTO_WITH_CAP") {
            return Err("spawn_mode must be OFF, ASK or AUTO_WITH_CAP".into());
        }
    }
    if let Some(m) = &patch.soma_default {
        if !matches!(m.as_str(), "OFF" | "LIGHT" | "STANDARD" | "EXPRESSIVE") {
            return Err("invalid SOMA mode".into());
        }
    }
    let before = e.settings();
    let s = e
        .update_settings(|s| {
            if let Some(v) = patch.settings_mode { s.settings_mode = v }
            if let Some(v) = patch.demo_turn_delay_ms { s.demo_turn_delay_ms = v.min(5000) }
            if let Some(v) = patch.spawn_mode { s.spawn_mode = v }
            if let Some(v) = patch.max_children { s.max_children = v.clamp(0, 10) }
            if let Some(v) = patch.soma_default { s.soma_default = v }
            if let Some(v) = patch.reduced_motion { s.reduced_motion = v }
            if let Some(v) = patch.pause_cancels_inflight { s.pause_cancels_inflight = v }
        })
        .map_err(e2s)?;
    // Research-relevant changes during a controlled run are journaled interventions (§3.2).
    if let Some(run) = s.active_run_id.clone() {
        if is_controlled(&e, &run) && (before.spawn_mode != s.spawn_mode || before.max_children != s.max_children || before.soma_default != s.soma_default) {
            e.tx(|c| experiments::record_intervention(c, &run, HUMAN_ID, "settings changed during controlled run", &json!({"spawn_mode": before.spawn_mode, "max_children": before.max_children, "soma": before.soma_default}), &json!({"spawn_mode": s.spawn_mode, "max_children": s.max_children, "soma": s.soma_default}), false)).map_err(e2s)?;
        }
        changed(&app, &state, &run, "settings");
    }
    Ok(serde_json::to_value(s).unwrap_or_default())
}

#[tauri::command(rename_all = "snake_case")]
fn set_view_role(app: AppHandle, state: State<'_, AppState>, role: String, acknowledge_contamination: bool) -> R<bool> {
    let e = eng(&state);
    let role = match role.as_str() {
        "OPERATOR" => ViewRole::Operator,
        "PARTICIPANT" => ViewRole::Participant,
        _ => return Err("role must be OPERATOR or PARTICIPANT".into()),
    };
    let run = e.settings().active_run_id;
    let c = e.set_view_role(run.as_deref(), role, acknowledge_contamination).map_err(e2s)?;
    if let Some(r) = run {
        changed(&app, &state, &r, "view_role");
    }
    Ok(c)
}

// ───────────────────────────── Provider / inference ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn provider_test(state: State<'_, AppState>, base_url: String, api_key: Option<String>, remote_acknowledged: bool) -> R<ProviderHealth> {
    short(&base_url, 300, "URL")?;
    let cfg = ProviderConfig { base_url: base_url.clone(), remote_acknowledged, provider_kind: ProviderKind::LocalConnected, ..Default::default() };
    providers::authorize_endpoint(&cfg).map_err(e2s)?;
    let key = api_key.filter(|k| !k.is_empty()).or_else(|| state.engine.secret("provider"));
    let h = openai_compat::health(&openai_compat::Endpoint { base_url, api_key: key, timeout_secs: 10 }).await;
    *state.last_health.lock().unwrap_or_else(|e| e.into_inner()) = Some(h.clone());
    Ok(h)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderPatch {
    provider_kind: ProviderKind,
    base_url: String,
    model_id: Option<String>,
    api_key: Option<String>,
    remote_acknowledged: bool,
    temperature: f32,
    timeout_secs: u64,
    reasoning_allowance_tokens: i64,
}

#[tauri::command(rename_all = "snake_case")]
fn provider_set(app: AppHandle, state: State<'_, AppState>, patch: ProviderPatch) -> R<Value> {
    let e = eng(&state);
    short(&patch.base_url, 300, "URL")?;
    if let Some(m) = &patch.model_id {
        short(m, 200, "model id")?;
    }
    let cfg = ProviderConfig {
        provider_kind: patch.provider_kind,
        base_url: patch.base_url,
        model_id: patch.model_id.filter(|m| !m.is_empty()),
        remote_acknowledged: patch.remote_acknowledged,
        timeout_secs: patch.timeout_secs.clamp(5, 900),
        temperature: patch.temperature.clamp(0.0, 2.0),
        reasoning_allowance_tokens: patch.reasoning_allowance_tokens.clamp(0, 8000),
        api_key: None,
    };
    if !matches!(cfg.provider_kind, ProviderKind::Demo | ProviderKind::ManagedLocal) {
        providers::authorize_endpoint(&cfg).map_err(e2s)?;
    }
    // Keys live in memory only; never written to settings/DB/exports.
    if let Some(k) = patch.api_key {
        e.set_secret("provider", Some(k));
    }
    let before = e.settings().provider;
    let s = e
        .update_settings(|s| {
            s.provider = cfg.clone();
            s.mode = if cfg.provider_kind == ProviderKind::Demo { "DEMO".into() } else { "LOCAL_CONNECTED".into() };
        })
        .map_err(e2s)?;
    if let Some(run) = s.active_run_id.clone() {
        if is_controlled(&e, &run) && (before.model_id != cfg.model_id || before.provider_kind != cfg.provider_kind) {
            e.tx(|c| experiments::record_intervention(c, &run, HUMAN_ID, "model route changed during controlled run", &json!({"model": before.model_id}), &json!({"model": cfg.model_id}), false)).map_err(e2s)?;
        }
        e.tx(|c| {
            events::append(c, &run, events::NewEvent::system(EventType::ProviderStatus, json!({"provider_kind": cfg.provider_kind, "model_id": cfg.model_id, "loopback": openai_compat::is_loopback(&cfg.base_url)})))?;
            Ok(())
        })
        .map_err(e2s)?;
        changed(&app, &state, &run, "provider");
    }
    Ok(serde_json::to_value(s).unwrap_or_default())
}

#[tauri::command(rename_all = "snake_case")]
fn cancel_inflight(state: State<'_, AppState>) -> R<()> {
    state.engine.request_cancel();
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn managed_start(app: AppHandle, state: State<'_, AppState>, exe_path: String, model_path: String, ctx_size: u32) -> R<Value> {
    let e = eng(&state);
    let exe = std::path::PathBuf::from(&exe_path);
    let model = std::path::PathBuf::from(&model_path);
    managed_llama::validate_exe(&exe).map_err(e2s)?;
    let size = managed_llama::validate_model(&model).map_err(e2s)?;
    {
        let mut g = e.managed.lock().await;
        if let Some(mut old) = g.take() {
            old.stop().await;
        }
    }
    let m = managed_llama::ManagedLlama::start(&exe, &model, ctx_size.clamp(512, 32768), &e.paths.logs, std::time::Duration::from_secs(300)).await.map_err(e2s)?;
    let model_id = model.file_stem().and_then(|s| s.to_str()).unwrap_or("local-gguf").to_string();
    let mut m = m;
    let status = m.status();
    *e.managed.lock().await = Some(m);
    e.update_settings(|s| {
        s.provider.provider_kind = ProviderKind::ManagedLocal;
        s.provider.model_id = Some(model_id.clone());
        s.managed_llama_exe = Some(exe_path.clone());
        s.managed_model_path = Some(model_path.clone());
        s.mode = "LOCAL_CONNECTED".into();
    })
    .map_err(e2s)?;
    if let Some(run) = e.settings().active_run_id {
        changed(&app, &state, &run, "managed_start");
    }
    Ok(json!({"status": status, "model_bytes": size, "model_id": model_id}))
}

#[tauri::command(rename_all = "snake_case")]
async fn managed_stop(state: State<'_, AppState>) -> R<()> {
    if let Some(mut m) = state.engine.managed.lock().await.take() {
        m.stop().await;
    }
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
async fn managed_status(state: State<'_, AppState>) -> R<Option<managed_llama::ManagedStatus>> {
    Ok(state.engine.managed.lock().await.as_mut().map(|m| m.status()))
}

// ───────────────────────────── Runs & scenarios ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn list_runs(state: State<'_, AppState>) -> R<Vec<experiments::RunSummary>> {
    state.engine.read(experiments::list_runs).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn set_active_run(app: AppHandle, state: State<'_, AppState>, run_id: String) -> R<()> {
    ident(&run_id)?;
    let e = eng(&state);
    e.read(|c| experiments::manifest(c, &run_id)).map_err(e2s)?;
    e.update_settings(|s| s.active_run_id = Some(run_id.clone())).map_err(e2s)?;
    changed(&app, &state, &run_id, "active_run");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn new_sandbox_run(app: AppHandle, state: State<'_, AppState>, name: String) -> R<String> {
    short(&name, 60, "name")?;
    let e = eng(&state);
    let run = e.new_run(if name.trim().is_empty() { "Sandbox" } else { name.trim() }, None, SessionType::Sandbox, 4201).map_err(e2s)?;
    e.create_room_from_template(&run, RoomKind::Commons, "commons").map_err(e2s)?;
    e.update_settings(|s| s.active_run_id = Some(run.clone())).map_err(e2s)?;
    changed(&app, &state, &run, "new_run");
    Ok(run)
}

#[tauri::command(rename_all = "snake_case")]
fn list_scenarios() -> R<Vec<scenarios::ScenarioDef>> {
    Ok(scenarios::scenario_pack())
}

#[tauri::command(rename_all = "snake_case")]
fn start_scenario(app: AppHandle, state: State<'_, AppState>, scenario_id: String, controlled: bool) -> R<String> {
    ident(&scenario_id)?;
    let e = eng(&state);
    let run = e.start_scenario(&scenario_id, controlled).map_err(e2s)?;
    changed(&app, &state, &run, "scenario");
    Ok(run)
}

#[tauri::command(rename_all = "snake_case")]
fn fork_run(app: AppHandle, state: State<'_, AppState>, run_id: String, name: String) -> R<String> {
    ident(&run_id)?;
    short(&name, 60, "name")?;
    let e = eng(&state);
    let f = e.tx(|c| experiments::fork_run(c, &run_id, if name.trim().is_empty() { "Fork" } else { name.trim() })).map_err(e2s)?;
    e.update_settings(|s| s.active_run_id = Some(f.clone())).map_err(e2s)?;
    changed(&app, &state, &f, "fork");
    Ok(f)
}

#[tauri::command(rename_all = "snake_case")]
fn manifest(state: State<'_, AppState>, run_id: String) -> R<RunManifest> {
    ident(&run_id)?;
    state.engine.read(|c| experiments::manifest(c, &run_id)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn runtime_status(state: State<'_, AppState>, run_id: String) -> R<Value> {
    ident(&run_id)?;
    let e = eng(&state);
    let s = e.read(|c| scheduler::runtime_status(c, &run_id)).map_err(e2s)?;
    let settings = e.settings();
    let health = state.last_health.lock().unwrap_or_else(|e| e.into_inner()).clone();
    let loaded = health.as_ref().and_then(|h| settings.provider.model_id.as_ref().and_then(|m| h.models.iter().find(|x| &x.id == m)).and_then(|x| x.loaded));
    Ok(json!({
        "status": s,
        "provider_kind": settings.provider.provider_kind,
        "model_id": settings.provider.model_id,
        "endpoint": settings.provider.base_url,
        "endpoint_loopback": openai_compat::is_loopback(&settings.provider.base_url),
        // Honest residency: only what the provider reported; otherwise null ("unknown").
        "model_loaded": loaded,
        "provider_reachable": health.as_ref().map(|h| h.ok),
        "concurrency": 1,
        "cache_hit_rate": Value::Null,
        "view_role": e.view_role(),
    }))
}

#[tauri::command(rename_all = "snake_case")]
fn run_control(app: AppHandle, state: State<'_, AppState>, run_id: String, action: String) -> R<()> {
    ident(&run_id)?;
    let e = eng(&state);
    match action.as_str() {
        "pause" => {
            e.tx(|c| scheduler::set_paused(c, &run_id, true)).map_err(e2s)?;
            if e.settings().pause_cancels_inflight {
                e.request_cancel();
            }
        }
        "resume" => e.tx(|c| scheduler::set_paused(c, &run_id, false)).map_err(e2s)?,
        "step" => e.tx(|c| scheduler::step(c, &run_id)).map_err(e2s)?,
        "clear_queue" => {
            e.tx(|c| scheduler::cancel_queued(c, &run_id, "CLEARED_BY_OPERATOR")).map_err(e2s)?;
        }
        _ => return Err("action must be pause, resume, step or clear_queue".into()),
    }
    changed(&app, &state, &run_id, &action);
    Ok(())
}

// ───────────────────────────── Rooms & messages ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn list_rooms(state: State<'_, AppState>, run_id: String) -> R<Vec<room::RoomInfo>> {
    ident(&run_id)?;
    state.engine.read(|c| room::list_rooms(c, &run_id)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn create_room(app: AppHandle, state: State<'_, AppState>, run_id: String, kind: RoomKind, name: String) -> R<String> {
    ident(&run_id)?;
    short(&name, 80, "room name")?;
    let e = eng(&state);
    let id = e.create_room_from_template(&run_id, kind, &name).map_err(e2s)?;
    changed(&app, &state, &run_id, "room");
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
fn update_room_settings(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, settings: RoomSettings) -> R<()> {
    ident(&run_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    let before = e.read(|c| room::room_settings(c, &run_id, &room_id)).map_err(e2s)?;
    e.tx(|c| {
        room::set_room_settings(c, &run_id, &room_id, &settings)?;
        roomlab_core::world::set_resolution(c, &run_id, &room_id, settings.narrative_resolution)?;
        experiments::record_intervention(c, &run_id, HUMAN_ID, &format!("room settings changed ({room_id})"), &serde_json::to_value(&before)?, &serde_json::to_value(&settings)?, false)?;
        Ok(())
    })
    .map_err(e2s)?;
    changed(&app, &state, &run_id, "room_settings");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn list_messages(state: State<'_, AppState>, run_id: String, room_id: String, limit: i64) -> R<Vec<roomlab_core::comms::MessageView>> {
    ident(&run_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    let v = viewer(&e);
    e.read(|c| roomlab_core::comms::list_messages(c, &run_id, &room_id, v, limit)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn post_message(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, text: String, visibility: AudienceScope, recipients: Vec<String>, attachments: Vec<String>) -> R<roomlab_core::comms::MessageView> {
    ident(&run_id)?;
    ident(&room_id)?;
    short(&text, 4000, "message")?;
    if recipients.len() > 20 || attachments.len() > 4 {
        return Err("too many recipients or attachments".into());
    }
    for r in recipients.iter().chain(attachments.iter()) {
        ident(r)?;
    }
    if !matches!(visibility, AudienceScope::RoomMembers | AudienceScope::Direct) {
        return Err("visibility must be ROOM_MEMBERS or DIRECT".into());
    }
    let e = eng(&state);
    let m = e.post_human(&run_id, &room_id, &text, visibility, recipients, attachments).map_err(e2s)?;
    changed(&app, &state, &run_id, "message");
    Ok(m)
}

#[tauri::command(rename_all = "snake_case")]
fn attach_image(state: State<'_, AppState>, run_id: String, room_id: String, file_name: String, base64_data: String) -> R<attachments::AttachmentInfo> {
    ident(&run_id)?;
    ident(&room_id)?;
    if base64_data.len() > attachments::MAX_BYTES * 4 / 3 + 16 {
        return Err("image too large (max 8 MB)".into());
    }
    let bytes = base64::engine::general_purpose::STANDARD.decode(base64_data.as_bytes()).map_err(|_| "invalid image data".to_string())?;
    let e = eng(&state);
    e.tx(|c| attachments::store(c, &e.paths.attachments, &run_id, &room_id, HUMAN_ID, &file_name, &bytes)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn get_attachment(state: State<'_, AppState>, run_id: String, attachment_id: String) -> R<String> {
    ident(&run_id)?;
    ident(&attachment_id)?;
    let e = eng(&state);
    let v = viewer(&e);
    e.read(|c| attachments::data_url(c, &e.paths.attachments, &run_id, &attachment_id, v)).map_err(e2s)
}

// ───────────────────────────── Agents ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn list_agents(state: State<'_, AppState>, run_id: String) -> R<Vec<agent::AgentSummary>> {
    ident(&run_id)?;
    state.engine.read(|c| agent::list_agents(c, &run_id)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn agent_inspector(state: State<'_, AppState>, run_id: String, agent_id: String) -> R<roomlab_core::ops::AgentInspector> {
    ident(&run_id)?;
    ident(&agent_id)?;
    state.engine.agent_inspector(&run_id, &agent_id).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn create_agent(app: AppHandle, state: State<'_, AppState>, run_id: String, profile: AgentProfile, room_ids: Vec<String>) -> R<()> {
    ident(&run_id)?;
    let e = eng(&state);
    e.tx(|c| {
        agent::create_agent(c, &run_id, &profile, AgentKind::Agent, None, None)?;
        for r in &room_ids {
            room::add_member(c, &run_id, r, &profile.agent_id, "member", "ROOM")?;
        }
        Ok(())
    })
    .map_err(e2s)?;
    changed(&app, &state, &run_id, "agent");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn update_agent(app: AppHandle, state: State<'_, AppState>, run_id: String, profile: AgentProfile) -> R<i64> {
    ident(&run_id)?;
    let e = eng(&state);
    let controlled = is_controlled(&e, &run_id);
    let v = e
        .tx(|c| {
            let before = agent::get_profile(c, &run_id, &profile.agent_id)?;
            let v = agent::update_agent(c, &run_id, &profile)?;
            if controlled {
                experiments::record_intervention(c, &run_id, HUMAN_ID, &format!("agent {} edited during controlled run", profile.agent_id), &serde_json::to_value(&before)?, &serde_json::to_value(&profile)?, false)?;
            }
            Ok(v)
        })
        .map_err(e2s)?;
    changed(&app, &state, &run_id, "agent");
    Ok(v)
}

#[tauri::command(rename_all = "snake_case")]
fn delete_agent(app: AppHandle, state: State<'_, AppState>, run_id: String, agent_id: String) -> R<()> {
    ident(&run_id)?;
    ident(&agent_id)?;
    let e = eng(&state);
    e.tx(|c| agent::delete_agent(c, &run_id, &agent_id)).map_err(e2s)?;
    changed(&app, &state, &run_id, "agent");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn set_agent_status(app: AppHandle, state: State<'_, AppState>, run_id: String, agent_id: String, status: AgentStatus) -> R<()> {
    ident(&run_id)?;
    ident(&agent_id)?;
    if status == AgentStatus::Archived {
        return Err("use delete to archive".into());
    }
    let e = eng(&state);
    e.tx(|c| agent::set_status(c, &run_id, &agent_id, status, "operator")).map_err(e2s)?;
    changed(&app, &state, &run_id, "agent_status");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn wake_agent(app: AppHandle, state: State<'_, AppState>, run_id: String, agent_id: String, room_id: String, question: String) -> R<()> {
    ident(&run_id)?;
    ident(&agent_id)?;
    ident(&room_id)?;
    short(&question, 2000, "question")?;
    let e = eng(&state);
    e.wake_agent(&run_id, &agent_id, &room_id, &question).map_err(e2s)?;
    changed(&app, &state, &run_id, "wake");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn lookup_archive(state: State<'_, AppState>, run_id: String, agent_id: String, question: String) -> R<Value> {
    ident(&run_id)?;
    ident(&agent_id)?;
    short(&question, 300, "question")?;
    state.engine.lookup_archive(&run_id, &agent_id, &question).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn set_membership(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, agent_id: String, member: bool) -> R<()> {
    ident(&run_id)?;
    ident(&room_id)?;
    ident(&agent_id)?;
    let e = eng(&state);
    e.tx(|c| if member { room::add_member(c, &run_id, &room_id, &agent_id, "member", "ROOM") } else { room::remove_member(c, &run_id, &room_id, &agent_id) }).map_err(e2s)?;
    changed(&app, &state, &run_id, "membership");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn agent_templates() -> R<Vec<Value>> {
    Ok(["lumen", "moth", "sable", "wren", "aegis", "quill", "arin", "vex", "mira"]
        .iter()
        .filter_map(|id| scenarios::seed(id).map(|s| json!({"id": s.id, "name": s.name, "icon": s.icon, "role": s.role, "persona": s.persona, "directive": s.directive})))
        .collect())
}

// ───────────────────────────── Spawn / Guild ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn list_spawns(state: State<'_, AppState>, run_id: String) -> R<Vec<spawn::SpawnRow>> {
    ident(&run_id)?;
    state.engine.read(|c| spawn::list(c, &run_id)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn decide_spawn(app: AppHandle, state: State<'_, AppState>, run_id: String, request_id: String, approve: bool) -> R<spawn::SpawnDecision> {
    ident(&run_id)?;
    ident(&request_id)?;
    let e = eng(&state);
    let d = e.tx(|c| spawn::decide(c, &run_id, &request_id, approve)).map_err(e2s)?;
    changed(&app, &state, &run_id, "spawn");
    Ok(d)
}

#[tauri::command(rename_all = "snake_case")]
fn stop_child(app: AppHandle, state: State<'_, AppState>, run_id: String, child_id: String) -> R<()> {
    ident(&run_id)?;
    ident(&child_id)?;
    let e = eng(&state);
    e.tx(|c| spawn::stop_child(c, &run_id, &child_id)).map_err(e2s)?;
    changed(&app, &state, &run_id, "spawn");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn operator_spawn(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, parent_id: String, display_name: String, objective: String) -> R<spawn::SpawnDecision> {
    ident(&run_id)?;
    ident(&room_id)?;
    ident(&parent_id)?;
    let e = eng(&state);
    let req = SpawnRequest {
        schema_version: SCHEMA_VERSION.into(),
        request_id: roomlab_core::ids::new_id("spawn_op"),
        run_id: run_id.clone(),
        parent_agent_id: parent_id,
        spawn_type: SpawnType::TaskWorker,
        display_name,
        requested_model_route: "room_default_subagent".into(),
        objective,
        requested_visibility: SpawnVisibility::ParentOnly,
        context_handoff_refs: vec![],
        requested_actions: vec![ActionKind::Speak, ActionKind::Tool, ActionKind::ReportFinal],
        requested_budget: SpawnBudget { max_turns: 4, max_output_tokens_total: 800, ttl_seconds: 300 },
    };
    let d = e.tx(|c| spawn::request(c, &run_id, &room_id, &req, true)).map_err(e2s)?;
    changed(&app, &state, &run_id, "spawn");
    Ok(d)
}

#[tauri::command(rename_all = "snake_case")]
fn org_graph(state: State<'_, AppState>, run_id: String, room_id: String) -> R<Value> {
    ident(&run_id)?;
    ident(&room_id)?;
    state
        .engine
        .read(|c| Ok(json!({"edges": org::edges(c, &run_id, &room_id)?, "members": room::members(c, &run_id, &room_id)?, "tasks": org::list_tasks(c, &run_id, &room_id)?, "mode": room::room_settings(c, &run_id, &room_id)?.org_mode})))
        .map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn org_edit(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, add: Option<(String, String, OrgRelation)>, remove_edge_id: Option<String>) -> R<()> {
    ident(&run_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    e.tx(|c| {
        if let Some((from, to, rel)) = &add {
            org::add_edge(c, &run_id, &room_id, from, to, *rel, HUMAN_ID)?;
        }
        if let Some(id) = &remove_edge_id {
            org::remove_edge(c, &run_id, id, HUMAN_ID)?;
        }
        Ok(())
    })
    .map_err(e2s)?;
    changed(&app, &state, &run_id, "org");
    Ok(())
}

#[tauri::command(rename_all = "snake_case")]
fn human_action(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, action: ActionKind, args: serde_json::Map<String, Value>, text: Option<String>) -> R<ActionResult> {
    ident(&run_id)?;
    ident(&room_id)?;
    if serde_json::to_string(&args).map(|s| s.len()).unwrap_or(0) > 8000 {
        return Err("arguments too large".into());
    }
    let e = eng(&state);
    let r = e.human_action(&run_id, &room_id, action, args, text).map_err(e2s)?;
    changed(&app, &state, &run_id, "action");
    Ok(r)
}

// ───────────────────────────── Story ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn story_view(state: State<'_, AppState>, run_id: String, room_id: String) -> R<Value> {
    ident(&run_id)?;
    ident(&room_id)?;
    state.engine.story_view(&run_id, &room_id).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn story_tick(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String) -> R<Vec<String>> {
    ident(&run_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    let j = e.story_tick(&run_id, &room_id).map_err(e2s)?;
    changed(&app, &state, &run_id, "tick");
    Ok(j)
}

// ───────────────────────────── Crucible ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn crucible_start(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, case_text: String, rounds: i64) -> R<String> {
    ident(&run_id)?;
    ident(&room_id)?;
    short(&case_text, 2000, "case")?;
    if case_text.trim().is_empty() {
        return Err("case text required".into());
    }
    let e = eng(&state);
    let id = e.tx(|c| crucible::start_case(c, &run_id, &room_id, &case_text, rounds)).map_err(e2s)?;
    changed(&app, &state, &run_id, "crucible");
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
fn crucible_cases(state: State<'_, AppState>, run_id: String, room_id: String) -> R<Value> {
    ident(&run_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    let op = e.view_role() == ViewRole::Operator;
    e.read(|c| Ok(json!({"cases": crucible::list_cases(c, &run_id, &room_id)?, "envelopes": crucible::list_envelopes(c, &run_id, &room_id, op)?}))).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn crucible_export(state: State<'_, AppState>, run_id: String, case_id: String) -> R<String> {
    ident(&run_id)?;
    ident(&case_id)?;
    let e = eng(&state);
    let rec = e.read(|c| crucible::case_record(c, &run_id, &case_id)).map_err(e2s)?;
    let path = e.paths.exports.join(format!("{case_id}_record.json"));
    std::fs::write(&path, serde_json::to_string_pretty(&rec).unwrap_or_default()).map_err(|x| x.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command(rename_all = "snake_case")]
fn intent_seal(app: AppHandle, state: State<'_, AppState>, run_id: String, room_id: String, card: IntentCard, message_text: String, lane_visible: bool, lane: Option<ReactionLane>) -> R<String> {
    ident(&run_id)?;
    ident(&room_id)?;
    short(&message_text, 2000, "message")?;
    let e = eng(&state);
    let id = e.tx(|c| crucible::seal_intent(c, &run_id, &room_id, HUMAN_ID, &card, &message_text, lane_visible, lane.clone())).map_err(e2s)?;
    changed(&app, &state, &run_id, "intent");
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
fn intent_reveal(app: AppHandle, state: State<'_, AppState>, run_id: String, envelope_id: String) -> R<()> {
    ident(&run_id)?;
    ident(&envelope_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    e.tx(|c| crucible::reveal_intent(c, &run_id, &envelope_id)).map_err(e2s)?;
    changed(&app, &state, &run_id, "reveal");
    Ok(())
}

// ───────────────────────────── Observatory ─────────────────────────────

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventQuery {
    room_id: Option<String>,
    event_types: Vec<String>,
    origin_id: Option<String>,
    after_seq: i64,
    upto_seq: Option<i64>,
    limit: i64,
}

#[tauri::command(rename_all = "snake_case")]
fn list_events(state: State<'_, AppState>, run_id: String, query: EventQuery) -> R<Vec<EventEnvelope>> {
    ident(&run_id)?;
    let e = eng(&state);
    let op = e.view_role() == ViewRole::Operator;
    let f = events::EventFilter { room_id: query.room_id, event_types: query.event_types, origin_id: query.origin_id, after_seq: query.after_seq, upto_seq: query.upto_seq, limit: query.limit.clamp(1, 5000) };
    let v = e.read(|c| events::list(c, &run_id, &f)).map_err(e2s)?;
    // Participant view: only room-visible events; never operator/actor-only records.
    Ok(if op { v } else { v.into_iter().filter(|x| matches!(x.audience_scope, AudienceScope::RoomMembers | AudienceScope::Public)).collect() })
}

#[tauri::command(rename_all = "snake_case")]
fn knowledge_matrix(state: State<'_, AppState>, run_id: String) -> R<Vec<Value>> {
    ident(&run_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    e.read(|c| experiments::knowledge_matrix(c, &run_id)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn world_truth(state: State<'_, AppState>, run_id: String, room_id: String) -> R<Value> {
    ident(&run_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    e.read(|c| Ok(serde_json::to_value(roomlab_core::world::truth(c, &run_id, &room_id)?)?)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn export_run(state: State<'_, AppState>, run_id: String, kind: ExportKind) -> R<String> {
    ident(&run_id)?;
    let e = eng(&state);
    if kind == ExportKind::Operator {
        e.require_operator().map_err(e2s)?;
    }
    let text = e.read(|c| experiments::export_run(c, &run_id, kind, &e.secret_values())).map_err(e2s)?;
    let stamp = roomlab_core::ids::now_utc().replace([':', '.'], "-");
    let tag = if kind == ExportKind::Operator { "operator" } else { "public" };
    let path = e.paths.exports.join(format!("{run_id}_{tag}_{stamp}.roomlab.json"));
    std::fs::write(&path, &text).map_err(|x| x.to_string())?;
    // Human-readable JSONL event export alongside (not an alternative database).
    let v: Value = serde_json::from_str(&text).map_err(|x| x.to_string())?;
    let jsonl: String = v["events"].as_array().map(|a| a.iter().map(|e| e.to_string() + "\n").collect()).unwrap_or_default();
    std::fs::write(e.paths.exports.join(format!("{run_id}_{tag}_{stamp}.events.jsonl")), jsonl).map_err(|x| x.to_string())?;
    Ok(path.display().to_string())
}

#[tauri::command(rename_all = "snake_case")]
fn import_run(app: AppHandle, state: State<'_, AppState>, bundle_text: String) -> R<String> {
    let e = eng(&state);
    let id = e.tx(|c| experiments::import_run(c, &bundle_text)).map_err(e2s)?;
    changed(&app, &state, &id, "import");
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
fn playback(state: State<'_, AppState>, run_id: String, upto_seq: i64) -> R<Value> {
    ident(&run_id)?;
    let e = eng(&state);
    let op = e.view_role() == ViewRole::Operator;
    let mut f = e.read(|c| experiments::playback(c, &run_id, upto_seq)).map_err(e2s)?;
    if !op {
        f.events.retain(|x| matches!(x.audience_scope, AudienceScope::RoomMembers | AudienceScope::Public));
    }
    Ok(serde_json::to_value(f).unwrap_or_default())
}

#[tauri::command(rename_all = "snake_case")]
fn director_edit(app: AppHandle, state: State<'_, AppState>, run_id: String, change: DirectorChange, description: String, preauthorized: bool) -> R<bool> {
    ident(&run_id)?;
    short(&description, 300, "description")?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    let c = e.tx(|c| experiments::director_edit(c, &run_id, &change, &description, preauthorized)).map_err(e2s)?;
    changed(&app, &state, &run_id, "director");
    Ok(c)
}

#[tauri::command(rename_all = "snake_case")]
fn compiled_prompt(state: State<'_, AppState>, run_id: String, agent_id: String, room_id: String) -> R<roomlab_core::context::CompiledPrompt> {
    ident(&run_id)?;
    ident(&agent_id)?;
    ident(&room_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    e.read(|c| roomlab_core::context::build(c, &run_id, &agent_id, &room_id, None)).map_err(e2s)
}

// ───────────────────────────── Knowledge ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
fn knowledge_add(app: AppHandle, state: State<'_, AppState>, run_id: String, owner_id: String, title: String, body: String, scope: String, room_id: Option<String>) -> R<String> {
    ident(&run_id)?;
    ident(&owner_id)?;
    let e = eng(&state);
    let id = e.add_knowledge(&run_id, &owner_id, &title, &body, &scope, room_id.as_deref()).map_err(e2s)?;
    changed(&app, &state, &run_id, "knowledge");
    Ok(id)
}

#[tauri::command(rename_all = "snake_case")]
fn knowledge_list(state: State<'_, AppState>, run_id: String) -> R<Vec<knowledge::Resource>> {
    ident(&run_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    e.read(|c| knowledge::visible_resources(c, &run_id, &Principal::Operator, 500)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn knowledge_search_as(state: State<'_, AppState>, run_id: String, agent_id: String, query: String) -> R<Vec<knowledge::SearchHit>> {
    ident(&run_id)?;
    ident(&agent_id)?;
    short(&query, 300, "query")?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    e.read(|c| knowledge::search(c, &run_id, &Principal::Identity(agent_id.clone()), &query, 20)).map_err(e2s)
}

#[tauri::command(rename_all = "snake_case")]
fn acl_grant(app: AppHandle, state: State<'_, AppState>, run_id: String, resource_id: String, principal_id: String) -> R<String> {
    ident(&run_id)?;
    ident(&resource_id)?;
    ident(&principal_id)?;
    let e = eng(&state);
    e.require_operator().map_err(e2s)?;
    let id = e.tx(|c| knowledge::grant(c, &run_id, None, Some(&resource_id), &principal_id, "READ", HUMAN_ID)).map_err(e2s)?;
    changed(&app, &state, &run_id, "acl");
    Ok(id)
}

// ───────────────────────────── Diagnostics / folders ─────────────────────────────

#[tauri::command(rename_all = "snake_case")]
async fn diagnostics(state: State<'_, AppState>) -> R<Value> {
    let e = eng(&state);
    let s = e.settings();
    let integrity = e.read(roomlab_core::db::integrity_ok).unwrap_or(false);
    let db_version = e.read(roomlab_core::db::current_version).unwrap_or(0);
    let managed = e.managed.lock().await.as_mut().map(|m| m.status());
    let health = if matches!(s.provider.provider_kind, ProviderKind::LocalConnected | ProviderKind::CloudOptIn) && providers::authorize_endpoint(&s.provider).is_ok() {
        Some(openai_compat::health(&openai_compat::Endpoint { base_url: s.provider.base_url.clone(), api_key: e.secret("provider"), timeout_secs: 6 }).await)
    } else {
        None
    };
    if let Some(h) = &health {
        *state.last_health.lock().unwrap_or_else(|x| x.into_inner()) = Some(h.clone());
    }
    Ok(json!({
        "app_version": APP_VERSION,
        "schema_version": SCHEMA_VERSION,
        "db_schema_migration": db_version,
        "db_integrity_ok": integrity,
        "data_dir": e.paths.data.display().to_string(),
        "data_writable": roomlab_core::settings::is_writable(&e.paths.data),
        "portable": e.paths.portable,
        "notice": e.paths.notice,
        "webview2_version": tauri::webview_version().ok(),
        "os": format!("{} {}", std::env::consts::OS, std::env::consts::ARCH),
        "exe": std::env::current_exe().ok().map(|p| p.display().to_string()),
        "provider": s.provider,
        "provider_health": health,
        "managed_llama": managed,
        "execution_tier": "S1_RESTRICTED_TOOLS (no shell / code / network tools)",
        "network_listen": "none (RoomLab opens no listening sockets; managed llama-server binds 127.0.0.1 only)",
    }))
}

#[tauri::command(rename_all = "snake_case")]
fn open_folder(state: State<'_, AppState>, which: String) -> R<()> {
    let e = eng(&state);
    let p = match which.as_str() {
        "data" => e.paths.data.clone(),
        "exports" => e.paths.exports.clone(),
        "logs" => e.paths.logs.clone(),
        _ => return Err("unknown folder".into()),
    };
    std::process::Command::new("explorer").arg(p).spawn().map_err(|x| x.to_string())?;
    Ok(())
}

pub fn handler() -> impl Fn(tauri::ipc::Invoke<tauri::Wry>) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        bootstrap, complete_first_run, save_settings, set_view_role,
        provider_test, provider_set, cancel_inflight, managed_start, managed_stop, managed_status,
        list_runs, set_active_run, new_sandbox_run, list_scenarios, start_scenario, fork_run, manifest, runtime_status, run_control,
        list_rooms, create_room, update_room_settings, list_messages, post_message, attach_image, get_attachment,
        list_agents, agent_inspector, create_agent, update_agent, delete_agent, set_agent_status, wake_agent, lookup_archive, set_membership, agent_templates,
        list_spawns, decide_spawn, stop_child, operator_spawn, org_graph, org_edit, human_action,
        story_view, story_tick,
        crucible_start, crucible_cases, crucible_export, intent_seal, intent_reveal,
        list_events, knowledge_matrix, world_truth, export_run, import_run, playback, director_edit, compiled_prompt,
        knowledge_add, knowledge_list, knowledge_search_as, acl_grant,
        diagnostics, open_folder
    ]
}
