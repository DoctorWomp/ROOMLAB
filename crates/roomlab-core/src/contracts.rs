//! Versioned wire contracts (§13). Every public contract uses `deny_unknown_fields`
//! and closed enums so invalid payloads are rejected rather than guessed at.
//! JSON Schema files in `contracts/` are generated from these types by `roomlab-schemas`.

use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{CoreError, Result};

pub const SCHEMA_VERSION: &str = "1.0";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn enum_str<T: Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(Value::String(s)) => s,
        _ => String::new(),
    }
}

pub fn parse_enum<T: DeserializeOwned>(s: &str) -> Result<T> {
    serde_json::from_value(Value::String(s.to_string()))
        .map_err(|_| CoreError::Invalid(format!("unknown enum value '{s}'")))
}

macro_rules! screaming_enum {
    ($(#[$m:meta])* $name:ident { $($v:ident),* $(,)? }) => {
        $(#[$m])*
        #[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
        #[serde(rename_all = "SCREAMING_SNAKE_CASE")]
        pub enum $name { $($v),* }
        impl $name {
            pub fn as_str(&self) -> String { enum_str(self) }
        }
    };
}

screaming_enum!(OriginKind { Human, Agent, Narrator, World, Tool, System });
screaming_enum!(AudienceScope { RoomMembers, Direct, ParentOnly, Location, OperatorOnly, ActorOnly, Public });
screaming_enum!(Modality { Text, Image, Mixed });
screaming_enum!(RenderStatus { Draft, Final });
screaming_enum!(ObsChannel { RoomChat, Direct, WorldLocal, Task, System, Reveal });
screaming_enum!(ObsKind {
    Message, LocationView, VisibleObjectState, ActionSeen, Sound, ActionResult, TaskBrief,
    SpawnResult, Reveal, Narration, KnowledgeGrant, Report
});
screaming_enum!(ObsStatus { Observed, Reported, Inferred, Degraded, Missing, Expired, Unknown });
screaming_enum!(ActionKind {
    Look, Inspect, Move, Speak, Whisper, Take, Drop, Use, Wait, Ask, RequestSpawn, NoOp, Summon,
    Tool, ReportProgress, ReportFinal, AskParent, AssignTask, UpdateTask
});
screaming_enum!(ReasonCode {
    Ok, InvalidSchema, ActorNotActive, NotMember, TargetUnknown, NotVisible, CapabilityDenied,
    AclDenied, StaleState, WorldRuleDenied, BudgetExhausted, ModelUnavailable, SpawnDepthExceeded,
    SpawnCapReached, UnsafeToolDisabled, RateLimited, PendingApproval, Queued
});
screaming_enum!(SpawnType { TaskWorker, RoomParticipant, WorldEntity });
screaming_enum!(SpawnState {
    Requested, Rejected, AwaitApproval, Queued, Initializing, Active, Suspended, Completing, Archived
});
screaming_enum!(SpawnPolicy { Disabled, ApprovalRequired, UniversalCapped, Delegated, WorldGated });
screaming_enum!(SpawnVisibility { ParentOnly, Room });
screaming_enum!(RoomKind { Commons, Guild, Story, Crucible });
screaming_enum!(AgentKind { Human, Agent, Narrator, WorldEntity });
screaming_enum!(AgentStatus { Active, Dormant, Archived });
screaming_enum!(SomaMode { Off, Light, Standard, Expressive });
screaming_enum!(Confidence { Commit, Lean, HeldOpen });
screaming_enum!(LaneOwner { SomaExpression, Character, ReaderHypothetical });
screaming_enum!(TaskStatus { New, InProgress, Blocked, ReadyForReview, Verified, Abandoned });
screaming_enum!(OrgMode { Manual, Assisted, SelfOrganizing });
screaming_enum!(SessionType { Sandbox, ControlledTrial });
screaming_enum!(RunMode { Live, Playback, Fork, Rerun });
screaming_enum!(ProviderKind { Demo, LocalConnected, ManagedLocal, CloudOptIn });
screaming_enum!(NarrativeResolution { Minimal, Balanced, Cinematic });
screaming_enum!(KnowledgeKind {
    Note, Document, WorldTruth, Observation, Reported, Belief, Inference, ReviewedFinding,
    HiddenTestKey, LoreExample
});
screaming_enum!(ResourceVisibility { Private, Room, Public, Sealed, OperatorOnly });
screaming_enum!(TurnOutcome { OfferedTurn, SuppressedByPolicy, Queued, Generated, ExplicitNoop, Failed, Coalesced });
screaming_enum!(EventType {
    RunCreated, RoomCreated, AgentCreated, AgentUpdated, AgentStatusChanged, MembershipChanged,
    MessageDelivered, ActionExecuted, ActionRejected, WorldStateChanged, Narration, TurnOutcome,
    JobQueued, JobFinished, InferenceCompleted, InferenceFailed, SpawnStateChanged, TaskUpdated,
    OrgChanged, KnowledgeAdded, AclChanged, ToolResult, Intervention, RunPaused, RunResumed,
    RunStepped, IntentSealed, IntentRevealed, CrucibleRound, ProviderStatus, ViewRoleChanged,
    AttachmentAdded, SettingsChanged, SystemNotice, BeliefUpdated
});

/// Cognition levels (§8.3). Serialized as `L0_SCRIPTED` etc.
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Cognition {
    #[serde(rename = "L0_SCRIPTED")]
    L0Scripted,
    #[serde(rename = "L1_REACTIVE")]
    L1Reactive,
    #[serde(rename = "L2_AGENTIC")]
    L2Agentic,
    #[serde(rename = "L3_CONTINUOUS")]
    L3Continuous,
}
impl Cognition {
    pub fn as_str(&self) -> String { enum_str(self) }
    pub fn uses_model(&self) -> bool { !matches!(self, Cognition::L0Scripted) }
}

/// Org relations are lower snake case in the spec (§4.1).
#[derive(Serialize, Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum OrgRelation { ReportsTo, AssignedTask, ReviewedBy, CanNominate, CanDelegate }
impl OrgRelation {
    pub fn as_str(&self) -> String { enum_str(self) }
}

// ───────────────────────────── Event envelope (§13.2) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    pub kind: OriginKind,
    pub id: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct EventEnvelope {
    pub schema_version: String,
    pub event_id: String,
    pub run_id: String,
    pub room_id: Option<String>,
    pub seq: i64,
    pub sim_time_ms: i64,
    pub wall_time_utc: String,
    pub event_type: EventType,
    pub origin: Origin,
    pub audience_scope: AudienceScope,
    pub payload: Value,
    pub parent_event_ids: Vec<String>,
}

// ───────────────────────────── Messages (§13.3) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ReactionLane {
    pub owner: LaneOwner,
    /// Required when owner is CHARACTER (e.g. "Arin").
    #[serde(default)]
    pub owner_name: Option<String>,
    pub steps: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct MessageRecord {
    pub message_id: String,
    pub room_id: String,
    pub channel_id: String,
    pub author_id: String,
    pub author_kind: OriginKind,
    pub recipient_ids: Vec<String>,
    pub visibility: AudienceScope,
    pub text: String,
    pub attachments: Vec<String>,
    pub modality: Modality,
    pub source_event_id: String,
    pub render_status: RenderStatus,
    #[serde(default)]
    pub reaction_lane: Option<ReactionLane>,
    #[serde(default)]
    pub confidence: Option<Confidence>,
    /// True when produced by a deterministic Demo (L0) policy. Rendered with a `DEMO` label.
    #[serde(default)]
    pub is_demo: bool,
    /// Model that actually produced the text, when provider-backed.
    #[serde(default)]
    pub model_id: Option<String>,
    #[serde(default)]
    pub seq: i64,
}

// ───────────────────────────── Observations (§13.4) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub observation_id: String,
    pub observer_id: String,
    pub source_event_id: String,
    pub channel: ObsChannel,
    pub kind: ObsKind,
    pub data: Value,
    pub status: ObsStatus,
    pub provenance: String,
    pub observed_at_sim_ms: i64,
    pub expires_at_sim_ms: Option<i64>,
}

// ───────────────────────────── Actions (§13.5) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionRequest {
    pub schema_version: String,
    pub request_id: String,
    pub agent_id: String,
    pub run_id: String,
    pub based_on_event_seq: i64,
    pub action: ActionKind,
    #[serde(default)]
    pub args: serde_json::Map<String, Value>,
    #[serde(default)]
    pub message_text: Option<String>,
    #[serde(default)]
    pub reaction_lane: Option<ReactionLane>,
    #[serde(default)]
    pub confidence: Option<Confidence>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ActionResult {
    pub request_id: String,
    pub accepted: bool,
    pub executed: bool,
    pub reason_code: ReasonCode,
    pub result_event_ids: Vec<String>,
    pub resolved_at_seq: i64,
    #[serde(default)]
    pub detail: Option<String>,
}

// ───────────────────────────── Spawn (§7.3) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpawnBudget {
    pub max_turns: i64,
    pub max_output_tokens_total: i64,
    pub ttl_seconds: i64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SpawnRequest {
    pub schema_version: String,
    pub request_id: String,
    pub run_id: String,
    pub parent_agent_id: String,
    pub spawn_type: SpawnType,
    pub display_name: String,
    pub requested_model_route: String,
    pub objective: String,
    pub requested_visibility: SpawnVisibility,
    pub context_handoff_refs: Vec<String>,
    pub requested_actions: Vec<ActionKind>,
    pub requested_budget: SpawnBudget,
}

// ───────────────────────────── Agent profile (§5.3) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub struct SocialAxes {
    pub sociability: f32,
    pub skepticism: f32,
    pub expressivity: f32,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CharacterSpec {
    pub role: String,
    pub motivation_core: String,
    /// FLEXIBLE | FIXED_FOR_TRIAL
    pub motivation_lock: String,
    pub secondary_drives: Vec<String>,
    /// SOFT | FIRM | STRICT
    pub role_rigidity: String,
    /// SPARSE | DETAILED
    pub description_detail: String,
    pub expressive_style: String,
    pub social_axes: SocialAxes,
    /// e.g. UPDATE_ON_EVIDENCE | RESISTANT | LOCKED
    #[serde(default = "default_belief_policy")]
    pub belief_update_policy: String,
    #[serde(default)]
    pub temperament_variance: f32,
}
fn default_belief_policy() -> String { "UPDATE_ON_EVIDENCE".into() }

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct InitialBelief {
    pub claim: String,
    pub status: String,
    pub source_ref: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct KnowledgeSpec {
    pub private_namespace: String,
    /// Requested mounts. NOT authoritative: the ACL table decides access.
    pub readable_mounts: Vec<String>,
    pub initial_beliefs: Vec<InitialBelief>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AutonomySpec {
    /// EVENT_DRIVEN | ON_MENTION | SCRIPTED
    pub policy: String,
    pub may_propose_new_goals: bool,
    pub may_choose_silence: bool,
    pub allowed_actions: Vec<ActionKind>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LimitsSpec {
    pub turn_output_tokens: i64,
    pub max_queued_calls: i64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AgentProfile {
    pub schema_version: String,
    pub agent_id: String,
    pub display_name: String,
    pub icon: String,
    pub model_route: String,
    pub cognition: Cognition,
    pub soma_mode: SomaMode,
    pub persona: String,
    pub directive: String,
    pub character: CharacterSpec,
    pub knowledge: KnowledgeSpec,
    pub autonomy: AutonomySpec,
    pub limits: LimitsSpec,
    /// Deterministic Demo policy id for L0 agents.
    #[serde(default)]
    pub demo_policy: Option<String>,
}

// ───────────────────────────── World (§6.2) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorldLocation {
    pub id: String,
    pub name: String,
    pub exits: Vec<String>,
    pub ambient: String,
    #[serde(default)]
    pub description: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct HiddenEdge {
    pub from: String,
    pub to: String,
    pub door_object_id: Option<String>,
    /// d20 difficulty for discovery via INSPECT in `from`.
    pub discover_dc: i64,
    /// Identities that already know this passage (authored private knowledge).
    #[serde(default)]
    pub known_by: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorldObject {
    pub id: String,
    pub name: String,
    pub location: String,
    /// DOCUMENT | DOOR | ITEM | FEATURE
    pub kind: String,
    /// VISIBLE | NOTICEABLE_IF_INSPECTED | HIDDEN
    pub visibility: String,
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub takeable: bool,
    #[serde(default)]
    pub description: String,
    /// For keys: the door id this unlocks.
    #[serde(default)]
    pub unlocks: Option<String>,
    /// For FEATURE objects: d20 difficulty for noticing their current state on arrival.
    #[serde(default)]
    pub notice_dc: Option<i64>,
    /// When this object is taken, set `feature_id`'s state (e.g. a clean rectangle in dust).
    #[serde(default)]
    pub trace_on_take: Option<TraceSpec>,
    /// Inspecting this other object reveals a NOTICEABLE_IF_INSPECTED object (e.g. map on shelf).
    #[serde(default)]
    pub found_with: Option<String>,
    /// Identities that already know where this object is (authored private knowledge).
    #[serde(default)]
    pub known_by: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TraceSpec {
    pub feature_id: String,
    pub state: String,
    /// What an observer perceives (no attribution of who caused it).
    pub cue: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorldCharacter {
    pub agent_id: String,
    pub location: String,
    #[serde(default)]
    pub inventory: Vec<String>,
    #[serde(default)]
    pub abilities: Vec<String>,
    #[serde(default)]
    pub spell_slots: i64,
    /// Perception bonus applied to observation checks.
    #[serde(default)]
    pub perception: i64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SecretFact {
    pub id: String,
    pub claim: String,
    pub owner_scope: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AmbientSound {
    pub location: String,
    pub description: String,
    /// Hidden cause; operator truth only.
    pub hidden_source: String,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WorldSpec {
    pub schema_version: String,
    pub world_id: String,
    pub title: String,
    pub genre: String,
    pub seed: i64,
    pub start_location: String,
    pub locations: Vec<WorldLocation>,
    #[serde(default)]
    pub hidden_edges: Vec<HiddenEdge>,
    pub objects: Vec<WorldObject>,
    pub characters: Vec<WorldCharacter>,
    #[serde(default)]
    pub secret_facts: Vec<SecretFact>,
    #[serde(default)]
    pub sounds: Vec<AmbientSound>,
    #[serde(default = "default_capacity")]
    pub inventory_capacity: i64,
}
fn default_capacity() -> i64 { 4 }

// ───────────────────────────── Experiments (§15) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConditionSpec {
    pub arm: String,
    pub key: String,
    pub value: Value,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RouteSnapshot {
    pub route_id: String,
    pub provider_kind: ProviderKind,
    pub model_id: Option<String>,
    pub endpoint_host: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunManifest {
    pub schema_version: String,
    pub app_version: String,
    pub run_id: String,
    pub name: String,
    pub scenario_id: Option<String>,
    pub session_type: SessionType,
    pub mode: RunMode,
    pub seed: i64,
    pub created_at: String,
    pub parent_run_id: Option<String>,
    pub conditions: Vec<ConditionSpec>,
    pub model_routes: Vec<RouteSnapshot>,
    pub budgets: RunBudgets,
    pub spawn_policy: SpawnPolicy,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunBudgets {
    pub output_tokens: i64,
    pub agent_turns: i64,
    pub max_chain_depth: i64,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct IntentCard {
    pub intended_stance: String,
    pub intended_lane: String,
    pub load_bearing_cue: String,
    pub predicted_misread: String,
    pub what_counts_as_hit: String,
}

// ───────────────────────────── Provider (§14) ─────────────────────────────

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderConfig {
    pub provider_kind: ProviderKind,
    pub base_url: String,
    pub model_id: Option<String>,
    /// Cloud/remote endpoints must be explicitly acknowledged by the operator.
    pub remote_acknowledged: bool,
    pub timeout_secs: u64,
    pub temperature: f32,
    /// Extra output tokens for models that spend tokens on hidden reasoning (e.g. Gemma 4, Qwen3).
    /// Added to each turn's visible-output ceiling; actual usage is still charged to the run budget.
    #[serde(default)]
    pub reasoning_allowance_tokens: i64,
    /// Never serialized; supplied per session from memory only.
    #[serde(skip)]
    #[schemars(skip)]
    pub api_key: Option<String>,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        ProviderConfig {
            provider_kind: ProviderKind::Demo,
            base_url: "http://127.0.0.1:1234/v1".into(),
            model_id: None,
            remote_acknowledged: false,
            timeout_secs: 120,
            temperature: 0.7,
            reasoning_allowance_tokens: 0,
            api_key: None,
        }
    }
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default)]
    pub loaded: Option<bool>,
    #[serde(default)]
    pub vision: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ProviderHealth {
    pub ok: bool,
    pub base_url: String,
    pub is_loopback: bool,
    pub models: Vec<ModelInfo>,
    pub diagnosis: String,
    pub latency_ms: Option<i64>,
}
