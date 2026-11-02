//! SQLite storage: forward-only migrations, WAL, foreign keys, backup-before-upgrade.

use std::path::Path;

use rusqlite::{params, Connection};

use crate::error::Result;
use crate::ids::now_utc;

/// Ordered, forward-only migrations. Never edit a shipped migration; append a new one.
pub const MIGRATIONS: &[(i64, &str)] = &[(1, MIGRATION_1)];

const MIGRATION_1: &str = r#"
CREATE TABLE workspaces (id TEXT PRIMARY KEY, name TEXT NOT NULL, created_at TEXT NOT NULL);

CREATE TABLE experiment_runs (
  id TEXT PRIMARY KEY,
  workspace_id TEXT NOT NULL REFERENCES workspaces(id),
  name TEXT NOT NULL,
  scenario_id TEXT,
  session_type TEXT NOT NULL CHECK (session_type IN ('SANDBOX','CONTROLLED_TRIAL')),
  mode TEXT NOT NULL CHECK (mode IN ('LIVE','PLAYBACK','FORK','RERUN')),
  seed INTEGER NOT NULL,
  manifest_json TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'ACTIVE',
  contaminated INTEGER NOT NULL DEFAULT 0,
  next_seq INTEGER NOT NULL DEFAULT 1,
  sim_time_ms INTEGER NOT NULL DEFAULT 0,
  paused INTEGER NOT NULL DEFAULT 0,
  step_tokens INTEGER NOT NULL DEFAULT 0,
  parent_run_id TEXT,
  budget_output_tokens INTEGER NOT NULL,
  used_output_tokens INTEGER NOT NULL DEFAULT 0 CHECK (used_output_tokens >= 0),
  budget_turns INTEGER NOT NULL,
  used_turns INTEGER NOT NULL DEFAULT 0 CHECK (used_turns >= 0),
  max_chain_depth INTEGER NOT NULL DEFAULT 3,
  spawn_policy TEXT NOT NULL DEFAULT 'APPROVAL_REQUIRED',
  created_at TEXT NOT NULL
);

CREATE TABLE experiment_conditions (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  id TEXT NOT NULL, arm TEXT NOT NULL, key TEXT NOT NULL, value_json TEXT NOT NULL,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE rooms (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('COMMONS','GUILD','STORY','CRUCIBLE')),
  name TEXT NOT NULL,
  settings_json TEXT NOT NULL,
  rule_revision INTEGER NOT NULL DEFAULT 1,
  archived INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE channels (
  run_id TEXT NOT NULL, id TEXT NOT NULL, room_id TEXT NOT NULL, name TEXT NOT NULL, kind TEXT NOT NULL,
  PRIMARY KEY (run_id, id),
  FOREIGN KEY (run_id, room_id) REFERENCES rooms(run_id, id) ON DELETE CASCADE
);

CREATE TABLE agents (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  display_name TEXT NOT NULL,
  icon TEXT NOT NULL,
  kind TEXT NOT NULL CHECK (kind IN ('HUMAN','AGENT','NARRATOR','WORLD_ENTITY')),
  cognition TEXT NOT NULL,
  model_route TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('ACTIVE','DORMANT','ARCHIVED')),
  parent_agent_id TEXT,
  spawn_type TEXT,
  soma_mode TEXT NOT NULL,
  demo_policy TEXT,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE agent_profiles (
  run_id TEXT NOT NULL, agent_id TEXT NOT NULL, version INTEGER NOT NULL, profile_json TEXT NOT NULL,
  PRIMARY KEY (run_id, agent_id),
  FOREIGN KEY (run_id, agent_id) REFERENCES agents(run_id, id) ON DELETE CASCADE
);

CREATE TABLE memberships (
  run_id TEXT NOT NULL, room_id TEXT NOT NULL, agent_id TEXT NOT NULL,
  role TEXT NOT NULL,
  can_post INTEGER NOT NULL DEFAULT 1,
  post_scope TEXT NOT NULL DEFAULT 'ROOM' CHECK (post_scope IN ('ROOM','PARENT_ONLY','NARRATOR','NONE')),
  joined_seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, room_id, agent_id),
  FOREIGN KEY (run_id, room_id) REFERENCES rooms(run_id, id) ON DELETE CASCADE,
  FOREIGN KEY (run_id, agent_id) REFERENCES agents(run_id, id) ON DELETE CASCADE
);

CREATE TABLE model_routes (
  id TEXT PRIMARY KEY,
  provider_kind TEXT NOT NULL,
  model_id TEXT,
  enabled INTEGER NOT NULL DEFAULT 1,
  description TEXT NOT NULL DEFAULT ''
);

CREATE TABLE agent_subscriptions (
  run_id TEXT NOT NULL, agent_id TEXT NOT NULL, room_id TEXT NOT NULL, trigger TEXT NOT NULL,
  PRIMARY KEY (run_id, agent_id, room_id, trigger)
);

CREATE TABLE events (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  event_id TEXT NOT NULL,
  seq INTEGER NOT NULL,
  room_id TEXT,
  sim_time_ms INTEGER NOT NULL,
  wall_time_utc TEXT NOT NULL,
  event_type TEXT NOT NULL,
  origin_kind TEXT NOT NULL,
  origin_id TEXT NOT NULL,
  audience_scope TEXT NOT NULL,
  payload_json TEXT NOT NULL,
  parent_event_ids_json TEXT NOT NULL,
  schema_version TEXT NOT NULL,
  PRIMARY KEY (run_id, event_id),
  UNIQUE (run_id, seq)
);

CREATE TABLE messages (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  message_id TEXT NOT NULL,
  room_id TEXT NOT NULL,
  channel_id TEXT NOT NULL,
  author_id TEXT NOT NULL,
  author_kind TEXT NOT NULL,
  recipient_ids_json TEXT NOT NULL,
  visibility TEXT NOT NULL,
  text TEXT NOT NULL,
  attachments_json TEXT NOT NULL,
  modality TEXT NOT NULL,
  source_event_id TEXT NOT NULL,
  render_status TEXT NOT NULL,
  reaction_lane_json TEXT,
  confidence TEXT,
  is_demo INTEGER NOT NULL DEFAULT 0,
  model_id TEXT,
  chain_depth INTEGER NOT NULL DEFAULT 0,
  seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, message_id)
);
CREATE INDEX messages_room ON messages(run_id, room_id, seq);

CREATE TABLE observations (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  observation_id TEXT NOT NULL,
  observer_id TEXT NOT NULL,
  room_id TEXT,
  source_event_id TEXT NOT NULL,
  channel TEXT NOT NULL,
  kind TEXT NOT NULL,
  data_json TEXT NOT NULL,
  status TEXT NOT NULL,
  provenance TEXT NOT NULL,
  observed_at_sim_ms INTEGER NOT NULL,
  expires_at_sim_ms INTEGER,
  seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, observation_id)
);
CREATE INDEX observations_observer ON observations(run_id, observer_id, seq);

CREATE TABLE beliefs (
  run_id TEXT NOT NULL, id TEXT NOT NULL, agent_id TEXT NOT NULL, claim TEXT NOT NULL,
  status TEXT NOT NULL, source_ref TEXT NOT NULL, updated_seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE knowledge_resources (
  run_id TEXT NOT NULL REFERENCES experiment_runs(id) ON DELETE CASCADE,
  id TEXT NOT NULL,
  namespace TEXT NOT NULL,
  owner_id TEXT NOT NULL,
  title TEXT NOT NULL,
  body TEXT NOT NULL,
  kind TEXT NOT NULL,
  visibility TEXT NOT NULL,
  version INTEGER NOT NULL DEFAULT 1,
  content_hash TEXT NOT NULL,
  source_id TEXT NOT NULL,
  confidential INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  updated_at TEXT NOT NULL,
  PRIMARY KEY (run_id, id)
);
CREATE VIRTUAL TABLE knowledge_fts USING fts5(title, body, run_id UNINDEXED, resource_id UNINDEXED, tokenize = 'unicode61');

CREATE TABLE acl_entries (
  run_id TEXT NOT NULL, id TEXT NOT NULL,
  namespace TEXT, resource_id TEXT,
  principal_id TEXT NOT NULL,
  permission TEXT NOT NULL CHECK (permission IN ('READ','WRITE')),
  granted_by TEXT NOT NULL,
  created_seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, id),
  CHECK (namespace IS NOT NULL OR resource_id IS NOT NULL)
);

CREATE TABLE tasks (
  run_id TEXT NOT NULL, cluster_id TEXT NOT NULL, room_id TEXT NOT NULL, goal TEXT NOT NULL,
  owner_agent_id TEXT NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('NEW','IN_PROGRESS','BLOCKED','READY_FOR_REVIEW','VERIFIED','ABANDONED')),
  assignees_json TEXT NOT NULL, priority INTEGER NOT NULL, dependencies_json TEXT NOT NULL,
  verification_criteria_json TEXT NOT NULL, human_action_needed INTEGER NOT NULL DEFAULT 0,
  notes_json TEXT NOT NULL DEFAULT '[]', updated_seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, cluster_id)
);

CREATE TABLE org_edges (
  run_id TEXT NOT NULL, id TEXT NOT NULL, room_id TEXT NOT NULL, from_id TEXT NOT NULL, to_id TEXT NOT NULL,
  relation TEXT NOT NULL CHECK (relation IN ('reports_to','assigned_task','reviewed_by','can_nominate','can_delegate')),
  PRIMARY KEY (run_id, id)
);

CREATE TABLE spawn_requests (
  run_id TEXT NOT NULL, request_id TEXT NOT NULL, parent_agent_id TEXT NOT NULL, room_id TEXT NOT NULL,
  spawn_type TEXT NOT NULL, payload_json TEXT NOT NULL, state TEXT NOT NULL, reason_code TEXT NOT NULL,
  effective_json TEXT, child_agent_id TEXT, created_seq INTEGER NOT NULL, updated_seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, request_id)
);

CREATE TABLE spawn_instances (
  run_id TEXT NOT NULL, child_agent_id TEXT NOT NULL, request_id TEXT NOT NULL, parent_agent_id TEXT NOT NULL,
  room_id TEXT NOT NULL, state TEXT NOT NULL,
  turns_used INTEGER NOT NULL DEFAULT 0, max_turns INTEGER NOT NULL,
  tokens_used INTEGER NOT NULL DEFAULT 0, max_tokens INTEGER NOT NULL,
  ttl_seconds INTEGER NOT NULL, started_wall_ms INTEGER NOT NULL, expires_wall_ms INTEGER NOT NULL,
  result_message_id TEXT, end_reason TEXT,
  PRIMARY KEY (run_id, child_agent_id),
  UNIQUE (run_id, request_id)
);

CREATE TABLE worlds (
  run_id TEXT NOT NULL, world_id TEXT NOT NULL, room_id TEXT NOT NULL, seed INTEGER NOT NULL,
  revision INTEGER NOT NULL DEFAULT 1, narrative_resolution TEXT NOT NULL, genre TEXT NOT NULL,
  title TEXT NOT NULL, spec_json TEXT NOT NULL, inventory_capacity INTEGER NOT NULL DEFAULT 4,
  PRIMARY KEY (run_id, world_id)
);

CREATE TABLE locations (
  run_id TEXT NOT NULL, world_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL,
  ambient TEXT NOT NULL, description TEXT NOT NULL,
  PRIMARY KEY (run_id, world_id, id)
);

CREATE TABLE world_edges (
  run_id TEXT NOT NULL, world_id TEXT NOT NULL, from_loc TEXT NOT NULL, to_loc TEXT NOT NULL,
  hidden INTEGER NOT NULL DEFAULT 0, discover_dc INTEGER NOT NULL DEFAULT 0,
  discovered_by_json TEXT NOT NULL DEFAULT '[]', door_entity_id TEXT,
  PRIMARY KEY (run_id, world_id, from_loc, to_loc)
);

CREATE TABLE world_entities (
  run_id TEXT NOT NULL, world_id TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL,
  kind TEXT NOT NULL, location_id TEXT, state TEXT NOT NULL DEFAULT '', visibility TEXT NOT NULL,
  props_json TEXT NOT NULL DEFAULT '{}', rev INTEGER NOT NULL DEFAULT 1,
  PRIMARY KEY (run_id, world_id, id)
);

CREATE TABLE inventories (
  run_id TEXT NOT NULL, world_id TEXT NOT NULL, holder_id TEXT NOT NULL, entity_id TEXT NOT NULL,
  acquired_seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, world_id, entity_id)
);

CREATE TABLE intent_envelopes (
  run_id TEXT NOT NULL, id TEXT NOT NULL, room_id TEXT NOT NULL, author_id TEXT NOT NULL,
  message_id TEXT, card_json TEXT NOT NULL, sealed INTEGER NOT NULL DEFAULT 1,
  created_seq INTEGER NOT NULL, revealed_seq INTEGER,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE provider_requests (
  run_id TEXT NOT NULL, id TEXT NOT NULL, job_id TEXT, agent_id TEXT NOT NULL, provider_kind TEXT NOT NULL,
  endpoint_host TEXT, requested_model TEXT, resolved_model TEXT, prompt_hash TEXT NOT NULL,
  context_refs_json TEXT NOT NULL, sampling_json TEXT NOT NULL, prompt_tokens_est INTEGER NOT NULL,
  usage_json TEXT, wall_start_ms INTEGER NOT NULL, wall_end_ms INTEGER, queue_wait_ms INTEGER,
  first_token_ms INTEGER, status TEXT NOT NULL, error TEXT, committed_event_id TEXT, repair_attempted INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE tool_results (
  run_id TEXT NOT NULL, id TEXT NOT NULL, agent_id TEXT NOT NULL, tool TEXT NOT NULL,
  args_json TEXT NOT NULL, result_json TEXT NOT NULL, reason_code TEXT NOT NULL, seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE interventions (
  run_id TEXT NOT NULL, id TEXT NOT NULL, actor TEXT NOT NULL, description TEXT NOT NULL,
  before_json TEXT NOT NULL, after_json TEXT NOT NULL, seq INTEGER NOT NULL, preauthorized INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE runtime_usage (
  run_id TEXT NOT NULL, agent_id TEXT NOT NULL,
  calls INTEGER NOT NULL DEFAULT 0, prompt_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0, demo_turns INTEGER NOT NULL DEFAULT 0,
  PRIMARY KEY (run_id, agent_id)
);

CREATE TABLE attachments (
  run_id TEXT NOT NULL, id TEXT NOT NULL, room_id TEXT NOT NULL, sha256 TEXT NOT NULL, mime TEXT NOT NULL,
  width INTEGER NOT NULL, height INTEGER NOT NULL, size_bytes INTEGER NOT NULL, original_name TEXT NOT NULL,
  stored_name TEXT NOT NULL, uploader_id TEXT NOT NULL, created_at TEXT NOT NULL,
  PRIMARY KEY (run_id, id)
);

CREATE TABLE jobs (
  run_id TEXT NOT NULL, job_id TEXT NOT NULL, agent_id TEXT NOT NULL, room_id TEXT NOT NULL,
  trigger_event_id TEXT NOT NULL, trigger_message_id TEXT, observation_cursor INTEGER NOT NULL,
  revision INTEGER NOT NULL, priority INTEGER NOT NULL, token_ceiling INTEGER NOT NULL,
  status TEXT NOT NULL CHECK (status IN ('QUEUED','RUNNING','DONE','CANCELLED','STALE','FAILED')),
  chain_depth INTEGER NOT NULL, reason TEXT NOT NULL, enqueued_seq INTEGER NOT NULL,
  enqueued_wall_ms INTEGER NOT NULL, started_wall_ms INTEGER, finished_wall_ms INTEGER, outcome TEXT,
  deadline_wall_ms INTEGER NOT NULL,
  PRIMARY KEY (run_id, job_id)
);
CREATE INDEX jobs_status ON jobs(run_id, status);

CREATE TABLE turn_log (
  run_id TEXT NOT NULL, id TEXT NOT NULL, agent_id TEXT NOT NULL, room_id TEXT, trigger_event_id TEXT,
  outcome TEXT NOT NULL, detail TEXT NOT NULL, seq INTEGER NOT NULL,
  PRIMARY KEY (run_id, id)
);
"#;

pub fn open(path: &Path) -> Result<Connection> {
    let existed = path.exists();
    let conn = Connection::open(path)?;
    configure(&conn)?;
    if existed {
        let current = current_version(&conn)?;
        let latest = MIGRATIONS.last().map(|m| m.0).unwrap_or(0);
        if current > 0 && current < latest {
            // Back up before any upgrade (forward-only migrations).
            let bak = path.with_extension(format!("sqlite.bak-v{current}"));
            conn.execute("VACUUM INTO ?1", params![bak.to_string_lossy()])?;
        }
    }
    migrate(&conn)?;
    Ok(conn)
}

pub fn open_memory() -> Result<Connection> {
    let conn = Connection::open_in_memory()?;
    configure(&conn)?;
    migrate(&conn)?;
    Ok(conn)
}

fn configure(conn: &Connection) -> Result<()> {
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // WAL may be unavailable for in-memory databases; ignore the returned mode.
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |r| r.get(0))?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.busy_timeout(std::time::Duration::from_secs(5))?;
    Ok(())
}

pub fn current_version(conn: &Connection) -> Result<i64> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, applied_at TEXT NOT NULL)",
        [],
    )?;
    Ok(conn.query_row("SELECT COALESCE(MAX(version),0) FROM schema_migrations", [], |r| r.get(0))?)
}

pub fn migrate(conn: &Connection) -> Result<()> {
    let current = current_version(conn)?;
    for (v, sql) in MIGRATIONS {
        if *v > current {
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let res = conn
                .execute_batch(sql)
                .and_then(|_| conn.execute("INSERT INTO schema_migrations(version, applied_at) VALUES (?1, ?2)", params![v, now_utc()]).map(|_| ()));
            match res {
                Ok(()) => conn.execute_batch("COMMIT")?,
                Err(e) => {
                    let _ = conn.execute_batch("ROLLBACK");
                    return Err(e.into());
                }
            }
        }
    }
    Ok(())
}

/// Consistency check used after crash/forced stop (T10).
pub fn integrity_ok(conn: &Connection) -> Result<bool> {
    let s: String = conn.query_row("PRAGMA integrity_check", [], |r| r.get(0))?;
    let fk: i64 = conn.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |r| r.get(0))?;
    Ok(s == "ok" && fk == 0)
}
