// Typed wrappers over the allowlisted Tauri commands (src-tauri/src/commands.rs).
// The frontend never decides permissions: it sends operator intents and renders backend state.
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type * as S from "./schemas";

const call = <T>(cmd: string, args?: Record<string, unknown>) => invoke<T>(cmd, args);

export const api = {
  bootstrap: () => call<S.Boot>("bootstrap"),
  completeFirstRun: (mode: "DEMO" | "LOCAL_CONNECTED") => call<string>("complete_first_run", { mode }),
  saveSettings: (patch: Partial<Pick<S.Settings, "settings_mode" | "demo_turn_delay_ms" | "spawn_mode" | "max_children" | "soma_default" | "reduced_motion" | "pause_cancels_inflight">>) =>
    call<S.Settings>("save_settings", { patch }),
  setViewRole: (role: S.ViewRole, acknowledge_contamination = false) => call<boolean>("set_view_role", { role, acknowledge_contamination }),

  providerTest: (base_url: string, api_key: string | null, remote_acknowledged: boolean) => call<S.ProviderHealth>("provider_test", { base_url, api_key, remote_acknowledged }),
  providerSet: (patch: S.ProviderConfig & { api_key?: string | null }) => call<S.Settings>("provider_set", { patch }),
  cancelInflight: () => call<void>("cancel_inflight"),
  managedStart: (exe_path: string, model_path: string, ctx_size: number) => call<Record<string, unknown>>("managed_start", { exe_path, model_path, ctx_size }),
  managedStop: () => call<void>("managed_stop"),
  managedStatus: () => call<Record<string, unknown> | null>("managed_status"),

  listRuns: () => call<S.RunSummary[]>("list_runs"),
  setActiveRun: (run_id: string) => call<void>("set_active_run", { run_id }),
  newSandboxRun: (name: string) => call<string>("new_sandbox_run", { name }),
  listScenarios: () => call<S.ScenarioDef[]>("list_scenarios"),
  startScenario: (scenario_id: string, controlled: boolean) => call<string>("start_scenario", { scenario_id, controlled }),
  forkRun: (run_id: string, name: string) => call<string>("fork_run", { run_id, name }),
  manifest: (run_id: string) => call<Record<string, unknown>>("manifest", { run_id }),
  runtimeStatus: (run_id: string) => call<S.RuntimeStatus>("runtime_status", { run_id }),
  runControl: (run_id: string, action: "pause" | "resume" | "step" | "clear_queue") => call<void>("run_control", { run_id, action }),

  listRooms: (run_id: string) => call<S.RoomInfo[]>("list_rooms", { run_id }),
  createRoom: (run_id: string, kind: S.RoomKind, name: string) => call<string>("create_room", { run_id, kind, name }),
  updateRoomSettings: (run_id: string, room_id: string, settings: S.RoomSettings) => call<void>("update_room_settings", { run_id, room_id, settings }),
  listMessages: (run_id: string, room_id: string, limit = 400) => call<S.MessageView[]>("list_messages", { run_id, room_id, limit }),
  postMessage: (run_id: string, room_id: string, text: string, visibility: "ROOM_MEMBERS" | "DIRECT", recipients: string[], attachments: string[]) =>
    call<S.MessageView>("post_message", { run_id, room_id, text, visibility, recipients, attachments }),
  attachImage: (run_id: string, room_id: string, file_name: string, base64_data: string) =>
    call<{ id: string; mime: string; width: number; height: number }>("attach_image", { run_id, room_id, file_name, base64_data }),
  getAttachment: (run_id: string, attachment_id: string) => call<string>("get_attachment", { run_id, attachment_id }),

  listAgents: (run_id: string) => call<S.AgentSummary[]>("list_agents", { run_id }),
  agentInspector: (run_id: string, agent_id: string) => call<Record<string, unknown> & { profile: S.AgentProfile; prompt_preview?: S.CompiledPrompt | null }>("agent_inspector", { run_id, agent_id }),
  createAgent: (run_id: string, profile: S.AgentProfile, room_ids: string[]) => call<void>("create_agent", { run_id, profile, room_ids }),
  updateAgent: (run_id: string, profile: S.AgentProfile) => call<number>("update_agent", { run_id, profile }),
  deleteAgent: (run_id: string, agent_id: string) => call<void>("delete_agent", { run_id, agent_id }),
  setAgentStatus: (run_id: string, agent_id: string, status: "ACTIVE" | "DORMANT") => call<void>("set_agent_status", { run_id, agent_id, status }),
  wakeAgent: (run_id: string, agent_id: string, room_id: string, question: string) => call<void>("wake_agent", { run_id, agent_id, room_id, question }),
  lookupArchive: (run_id: string, agent_id: string, question: string) => call<Record<string, unknown>>("lookup_archive", { run_id, agent_id, question }),
  setMembership: (run_id: string, room_id: string, agent_id: string, member: boolean) => call<void>("set_membership", { run_id, room_id, agent_id, member }),
  agentTemplates: () => call<{ id: string; name: string; icon: string; role: string; persona: string; directive: string }[]>("agent_templates"),

  listSpawns: (run_id: string) => call<S.SpawnRow[]>("list_spawns", { run_id }),
  decideSpawn: (run_id: string, request_id: string, approve: boolean) => call<Record<string, unknown>>("decide_spawn", { run_id, request_id, approve }),
  stopChild: (run_id: string, child_id: string) => call<void>("stop_child", { run_id, child_id }),
  operatorSpawn: (run_id: string, room_id: string, parent_id: string, display_name: string, objective: string) =>
    call<Record<string, unknown>>("operator_spawn", { run_id, room_id, parent_id, display_name, objective }),
  orgGraph: (run_id: string, room_id: string) => call<{ edges: S.OrgEdge[]; members: S.MemberInfo[]; tasks: S.TaskCluster[]; mode: string }>("org_graph", { run_id, room_id }),
  orgEdit: (run_id: string, room_id: string, add: [string, string, string] | null, remove_edge_id: string | null) => call<void>("org_edit", { run_id, room_id, add, remove_edge_id }),
  humanAction: (run_id: string, room_id: string, action: S.ActionKind, args: Record<string, unknown>, text: string | null = null) =>
    call<S.ActionResult>("human_action", { run_id, room_id, action, args, text }),

  storyView: (run_id: string, room_id: string) => call<Record<string, any>>("story_view", { run_id, room_id }),
  storyTick: (run_id: string, room_id: string) => call<string[]>("story_tick", { run_id, room_id }),

  crucibleStart: (run_id: string, room_id: string, case_text: string, rounds: number) => call<string>("crucible_start", { run_id, room_id, case_text, rounds }),
  crucibleCases: (run_id: string, room_id: string) => call<{ cases: Record<string, any>[]; envelopes: Record<string, any>[] }>("crucible_cases", { run_id, room_id }),
  crucibleExport: (run_id: string, case_id: string) => call<string>("crucible_export", { run_id, case_id }),
  intentSeal: (run_id: string, room_id: string, card: S.IntentCard, message_text: string, lane_visible: boolean, lane: S.ReactionLane | null) =>
    call<string>("intent_seal", { run_id, room_id, card, message_text, lane_visible, lane }),
  intentReveal: (run_id: string, envelope_id: string) => call<void>("intent_reveal", { run_id, envelope_id }),

  listEvents: (run_id: string, query: { room_id: string | null; event_types: string[]; origin_id: string | null; after_seq: number; upto_seq: number | null; limit: number }) =>
    call<S.EventEnvelope[]>("list_events", { run_id, query }),
  knowledgeMatrix: (run_id: string) => call<Record<string, any>[]>("knowledge_matrix", { run_id }),
  worldTruth: (run_id: string, room_id: string) => call<Record<string, any>>("world_truth", { run_id, room_id }),
  exportRun: (run_id: string, kind: "PUBLIC" | "OPERATOR") => call<string>("export_run", { run_id, kind }),
  importRun: (bundle_text: string) => call<string>("import_run", { bundle_text }),
  playback: (run_id: string, upto_seq: number) => call<{ upto_seq: number; max_seq: number; events: S.EventEnvelope[]; message_ids: string[]; note: string }>("playback", { run_id, upto_seq }),
  directorEdit: (run_id: string, change: Record<string, unknown>, description: string, preauthorized: boolean) => call<boolean>("director_edit", { run_id, change, description, preauthorized }),
  compiledPrompt: (run_id: string, agent_id: string, room_id: string) => call<S.CompiledPrompt>("compiled_prompt", { run_id, agent_id, room_id }),

  knowledgeAdd: (run_id: string, owner_id: string, title: string, body: string, scope: "PRIVATE" | "ROOM" | "PUBLIC", room_id: string | null) =>
    call<string>("knowledge_add", { run_id, owner_id, title, body, scope, room_id }),
  knowledgeList: (run_id: string) => call<Record<string, any>[]>("knowledge_list", { run_id }),
  knowledgeSearchAs: (run_id: string, agent_id: string, query: string) => call<Record<string, any>[]>("knowledge_search_as", { run_id, agent_id, query }),
  aclGrant: (run_id: string, resource_id: string, principal_id: string) => call<string>("acl_grant", { run_id, resource_id, principal_id }),

  diagnostics: () => call<Record<string, any>>("diagnostics"),
  openFolder: (which: "data" | "exports" | "logs") => call<void>("open_folder", { which }),
};

export function onChanged(cb: (p: { run_id: string; reason: string }) => void): Promise<UnlistenFn> {
  return listen<{ run_id: string; reason: string }>("roomlab://changed", (e) => cb(e.payload));
}
export function onDraft(cb: (d: S.DraftUpdate) => void): Promise<UnlistenFn> {
  return listen<S.DraftUpdate>("roomlab://draft", (e) => cb(e.payload));
}
export function onRunnerError(cb: (msg: string) => void): Promise<UnlistenFn> {
  return listen<string>("roomlab://error", (e) => cb(e.payload));
}

export function errText(e: unknown): string {
  return typeof e === "string" ? e : e instanceof Error ? e.message : JSON.stringify(e);
}
