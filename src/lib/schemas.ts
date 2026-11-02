// TypeScript mirrors of the Rust wire contracts (crates/roomlab-core/src/contracts.rs).
// JSON Schema files generated from the Rust types live in /contracts.

export type OriginKind = "HUMAN" | "AGENT" | "NARRATOR" | "WORLD" | "TOOL" | "SYSTEM";
export type AudienceScope = "ROOM_MEMBERS" | "DIRECT" | "PARENT_ONLY" | "LOCATION" | "OPERATOR_ONLY" | "ACTOR_ONLY" | "PUBLIC";
export type RoomKind = "COMMONS" | "GUILD" | "STORY" | "CRUCIBLE";
export type Confidence = "COMMIT" | "LEAN" | "HELD_OPEN";
export type LaneOwner = "SOMA_EXPRESSION" | "CHARACTER" | "READER_HYPOTHETICAL";
export type SomaMode = "OFF" | "LIGHT" | "STANDARD" | "EXPRESSIVE";
export type Cognition = "L0_SCRIPTED" | "L1_REACTIVE" | "L2_AGENTIC" | "L3_CONTINUOUS";
export type ProviderKind = "DEMO" | "LOCAL_CONNECTED" | "MANAGED_LOCAL" | "CLOUD_OPT_IN";
export type ViewRole = "OPERATOR" | "PARTICIPANT";
export type ActionKind =
  | "LOOK" | "INSPECT" | "MOVE" | "SPEAK" | "WHISPER" | "TAKE" | "DROP" | "USE" | "WAIT" | "ASK"
  | "REQUEST_SPAWN" | "NO_OP" | "SUMMON" | "TOOL" | "REPORT_PROGRESS" | "REPORT_FINAL" | "ASK_PARENT"
  | "ASSIGN_TASK" | "UPDATE_TASK";
export const ALL_ACTIONS: ActionKind[] = [
  "SPEAK", "ASK", "WHISPER", "NO_OP", "WAIT", "TOOL", "LOOK", "INSPECT", "MOVE", "TAKE", "DROP", "USE",
  "SUMMON", "REQUEST_SPAWN", "ASSIGN_TASK", "UPDATE_TASK", "REPORT_PROGRESS", "REPORT_FINAL", "ASK_PARENT",
];

export interface ReactionLane { owner: LaneOwner; owner_name?: string | null; steps: string[] }

export interface MessageView {
  message_id: string; room_id: string; channel_id: string; author_id: string; author_kind: OriginKind;
  recipient_ids: string[]; visibility: AudienceScope; text: string; attachments: string[]; modality: string;
  source_event_id: string; render_status: "DRAFT" | "FINAL"; reaction_lane?: ReactionLane | null;
  confidence?: Confidence | null; is_demo: boolean; model_id?: string | null; seq: number;
  author_name: string; author_icon: string; recipient_names: string[]; private: boolean; chain_depth: number;
}

export interface MemberInfo {
  agent_id: string; display_name: string; icon: string; kind: string; role: string; status: string;
  post_scope: string; cognition: Cognition; model_route: string; parent_agent_id?: string | null;
}

export interface RoomSettings {
  spawn_policy: "DISABLED" | "APPROVAL_REQUIRED" | "UNIVERSAL_CAPPED" | "DELEGATED" | "WORLD_GATED";
  max_active_children: number; max_children_per_parent: number; child_max_turns: number; child_ttl_seconds: number;
  child_max_output_tokens: number; plain_text_as_speak: boolean; narrative_resolution: "MINIMAL" | "BALANCED" | "CINEMATIC";
  unavailable_policy: string; org_mode: "MANUAL" | "ASSISTED" | "SELF_ORGANIZING"; default_model_route: string;
  soma_enabled: boolean; read_only: boolean;
}

export interface RoomInfo { id: string; kind: RoomKind; name: string; settings: RoomSettings; rule_revision: number; members: MemberInfo[] }

export interface AgentSummary {
  id: string; display_name: string; icon: string; kind: string; cognition: Cognition; model_route: string; status: string;
  soma_mode: SomaMode; parent_agent_id?: string | null; spawn_type?: string | null; role: string; queued_jobs: number;
  running_jobs: number; calls: number; output_tokens: number; demo_turns: number;
}

export interface AgentProfile {
  schema_version: string; agent_id: string; display_name: string; icon: string; model_route: string;
  cognition: Cognition; soma_mode: SomaMode; persona: string; directive: string;
  character: {
    role: string; motivation_core: string; motivation_lock: string; secondary_drives: string[]; role_rigidity: string;
    description_detail: string; expressive_style: string; social_axes: { sociability: number; skepticism: number; expressivity: number };
    belief_update_policy: string; temperament_variance: number;
  };
  knowledge: { private_namespace: string; readable_mounts: string[]; initial_beliefs: { claim: string; status: string; source_ref: string }[] };
  autonomy: { policy: string; may_propose_new_goals: boolean; may_choose_silence: boolean; allowed_actions: ActionKind[] };
  limits: { turn_output_tokens: number; max_queued_calls: number };
  demo_policy?: string | null;
}

export interface ProviderConfig {
  provider_kind: ProviderKind; base_url: string; model_id?: string | null; remote_acknowledged: boolean;
  timeout_secs: number; temperature: number; reasoning_allowance_tokens: number;
}

export interface Settings {
  first_run_complete: boolean; mode: string; settings_mode: "SIMPLE" | "ADVANCED"; active_run_id?: string | null;
  provider: ProviderConfig; demo_turn_delay_ms: number; reduced_motion: string; spawn_mode: "OFF" | "ASK" | "AUTO_WITH_CAP";
  max_children: number; soma_default: SomaMode; pause_cancels_inflight: boolean;
  managed_llama_exe?: string | null; managed_model_path?: string | null;
}

export interface Boot {
  app_version: string; schema_version: string; settings: Settings; data_dir: string; exports_dir: string;
  notice?: string | null; portable: boolean; view_role: ViewRole; webview_version?: string | null; secret_set: boolean;
}

export interface ModelInfo { id: string; loaded?: boolean | null; vision?: boolean | null }
export interface ProviderHealth { ok: boolean; base_url: string; is_loopback: boolean; models: ModelInfo[]; diagnosis: string; latency_ms?: number | null }

export interface RuntimeStatus {
  status: {
    run_id: string; run_name: string; session_type: string; mode: string; contaminated: boolean; paused: boolean;
    identities_active: number; identities_dormant: number; children_active: number; queued_jobs: number; running_jobs: number;
    failed_jobs: number; spawn_pending_approval: number; used_turns: number; budget_turns: number; used_output_tokens: number;
    budget_output_tokens: number; last_seq: number; sim_time_ms: number; errors: number;
  };
  provider_kind: ProviderKind; model_id?: string | null; endpoint: string; endpoint_loopback: boolean;
  model_loaded?: boolean | null; provider_reachable?: boolean | null; concurrency: number; view_role: ViewRole;
}

export interface RunSummary { id: string; name: string; scenario_id?: string | null; session_type: string; mode: string; contaminated: boolean; created_at: string; events: number; parent_run_id?: string | null }
export interface ScenarioDef { id: string; room: RoomKind; title: string; question: string; how_to_run: string; requires?: string | null }

export interface SpawnRow {
  request_id: string; parent_agent_id: string; parent_name: string; room_id: string; spawn_type: string; state: string;
  reason_code: string; display_name: string; objective: string; child_agent_id?: string | null; effective?: Record<string, unknown> | null;
  turns_used?: number | null; max_turns?: number | null; expires_wall_ms?: number | null; end_reason?: string | null;
}

export interface EventEnvelope {
  schema_version: string; event_id: string; run_id: string; room_id?: string | null; seq: number; sim_time_ms: number;
  wall_time_utc: string; event_type: string; origin: { kind: OriginKind; id: string }; audience_scope: AudienceScope;
  payload: Record<string, unknown>; parent_event_ids: string[];
}

export interface DraftUpdate { run_id: string; job_id: string; agent_id: string; agent_name: string; room_id: string; text: string; done: boolean }
export interface ActionResult { request_id: string; accepted: boolean; executed: boolean; reason_code: string; result_event_ids: string[]; resolved_at_seq: number; detail?: string | null }

export interface OrgEdge { id: string; room_id: string; from_id: string; to_id: string; relation: string }
export interface TaskCluster {
  cluster_id: string; room_id: string; goal: string; owner_agent_id: string; status: string; assignees: string[]; priority: number;
  dependencies: string[]; verification_criteria: string[]; human_action_needed: boolean; notes: { by: string; note: string; seq: number }[];
}

export interface IntentCard { intended_stance: string; intended_lane: string; load_bearing_cue: string; predicted_misread: string; what_counts_as_hit: string }

export interface CompiledPrompt { agent_id: string; segments: { label: string; text: string }[]; resource_refs: string[]; observation_count: number; token_estimate: number; template_hash: string; rule_revision: number }
