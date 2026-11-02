import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { ALL_ACTIONS, type ActionKind, type AgentProfile, type Cognition, type RoomInfo, type SomaMode } from "../lib/schemas";
import { Modal, useNotify } from "./ui";

const blank = (): AgentProfile => ({
  schema_version: "1.0", agent_id: "", display_name: "", icon: "🤖", model_route: "local/default", cognition: "L1_REACTIVE", soma_mode: "LIGHT",
  persona: "", directive: "",
  character: { role: "participant", motivation_core: "", motivation_lock: "FLEXIBLE", secondary_drives: [], role_rigidity: "SOFT", description_detail: "SPARSE", expressive_style: "plain", social_axes: { sociability: 0.5, skepticism: 0.5, expressivity: 0.4 }, belief_update_policy: "UPDATE_ON_EVIDENCE", temperament_variance: 0.1 },
  knowledge: { private_namespace: "", readable_mounts: [], initial_beliefs: [] },
  autonomy: { policy: "EVENT_DRIVEN", may_propose_new_goals: true, may_choose_silence: true, allowed_actions: ["SPEAK", "ASK", "WHISPER", "NO_OP", "WAIT", "TOOL"] },
  limits: { turn_output_tokens: 200, max_queued_calls: 1 },
  demo_policy: null,
});

const DEMO_POLICIES = ["lumen", "moth", "sable", "wren", "aegis", "quill", "arin", "vex", "mira", "echo"];

export default function AgentFoundry({ runId, rooms, agentId, onClose, onSaved }: { runId: string; rooms: RoomInfo[]; agentId: string | null; onClose: () => void; onSaved: () => void }) {
  const notify = useNotify();
  const [p, setP] = useState<AgentProfile>(blank());
  const [roomIds, setRoomIds] = useState<string[]>([]);
  const [templates, setTemplates] = useState<{ id: string; name: string; icon: string; role: string; persona: string; directive: string }[]>([]);
  const editing = agentId !== null;

  useEffect(() => {
    api.agentTemplates().then(setTemplates).catch(() => {});
    if (agentId) api.agentInspector(runId, agentId).then((d) => setP(d.profile)).catch((e) => notify(errText(e), true));
  }, [runId, agentId, notify]);

  const set = <K extends keyof AgentProfile>(k: K, v: AgentProfile[K]) => setP((x) => ({ ...x, [k]: v }));
  const setC = (k: keyof AgentProfile["character"], v: any) => setP((x) => ({ ...x, character: { ...x.character, [k]: v } }));
  const toggleAction = (a: ActionKind) => setP((x) => ({ ...x, autonomy: { ...x.autonomy, allowed_actions: x.autonomy.allowed_actions.includes(a) ? x.autonomy.allowed_actions.filter((y) => y !== a) : [...x.autonomy.allowed_actions, a] } }));
  const setCog = (c: Cognition) => setP((x) => ({ ...x, cognition: c, model_route: c === "L0_SCRIPTED" ? "demo" : x.model_route === "demo" ? "local/default" : x.model_route, demo_policy: c === "L0_SCRIPTED" ? x.demo_policy ?? "sable" : null }));

  const save = async () => {
    const id = p.agent_id.trim() || p.display_name.trim().toLowerCase().replace(/[^a-z0-9_-]+/g, "_").replace(/^_+|_+$/g, "").slice(0, 40);
    const prof: AgentProfile = { ...p, agent_id: id, knowledge: { ...p.knowledge, private_namespace: `agent/${id}/private`, readable_mounts: [`agent/${id}/private`] } };
    try {
      if (editing) await api.updateAgent(runId, prof);
      else await api.createAgent(runId, prof, roomIds);
      notify(editing ? "Saved (stable ID kept)" : "Agent created");
      onSaved();
    } catch (e) {
      notify(errText(e), true);
    }
  };

  return (
    <Modal title={editing ? `Edit ${p.display_name}` : "Agent Foundry — new agent"} onClose={onClose} wide>
      <div className="col">
        {!editing && templates.length > 0 && (
          <label className="field">Start from a template
            <select onChange={(e) => { const t = templates.find((x) => x.id === e.target.value); if (t) setP((x) => ({ ...x, display_name: t.name + " II", icon: t.icon, persona: t.persona, directive: t.directive, character: { ...x.character, role: t.role } })); }} defaultValue="">
              <option value="">— blank —</option>{templates.map((t) => <option key={t.id} value={t.id}>{t.icon} {t.name} · {t.role}</option>)}
            </select>
          </label>
        )}
        <div className="grid3">
          <label className="field">Display name<input value={p.display_name} onChange={(e) => set("display_name", e.target.value)} maxLength={40} /></label>
          <label className="field">Icon (emoji)<input value={p.icon} onChange={(e) => set("icon", e.target.value)} maxLength={8} /></label>
          <label className="field">Agent ID {editing ? "(stable, read-only)" : "(optional)"}<input value={p.agent_id} disabled={editing} onChange={(e) => set("agent_id", e.target.value)} placeholder="auto from name" /></label>
        </div>
        <div className="grid3">
          <label className="field">Cognition
            <select value={p.cognition} onChange={(e) => setCog(e.target.value as Cognition)}>
              <option value="L0_SCRIPTED">L0 scripted (Demo, no model)</option>
              <option value="L1_REACTIVE">L1 reactive (model on mention/direct)</option>
              <option value="L2_AGENTIC">L2 agentic (model, group turns)</option>
            </select>
          </label>
          {p.cognition === "L0_SCRIPTED" ? (
            <label className="field">Demo policy<select value={p.demo_policy ?? "sable"} onChange={(e) => set("demo_policy", e.target.value)}>{DEMO_POLICIES.map((d) => <option key={d}>{d}</option>)}</select></label>
          ) : (
            <label className="field">Model route<select value={p.model_route} onChange={(e) => set("model_route", e.target.value)}><option value="local/default">local/default (model chosen in Settings)</option></select></label>
          )}
          <label className="field">SOMA expression<select value={p.soma_mode} onChange={(e) => set("soma_mode", e.target.value as SomaMode)}>{["OFF", "LIGHT", "STANDARD", "EXPRESSIVE"].map((m) => <option key={m}>{m}</option>)}</select></label>
        </div>
        <div className="grid2">
          <label className="field">Role<input value={p.character.role} onChange={(e) => setC("role", e.target.value)} /></label>
          <label className="field">Core motivation<input value={p.character.motivation_core} onChange={(e) => setC("motivation_core", e.target.value)} /></label>
        </div>
        <label className="field">Persona (private to this agent's prompt)<textarea rows={3} value={p.persona} onChange={(e) => set("persona", e.target.value)} /></label>
        <label className="field">Current directive<textarea rows={2} value={p.directive} onChange={(e) => set("directive", e.target.value)} /></label>
        <h3>Descriptive precision vs behavioral rigidity (independent controls)</h3>
        <div className="grid3">
          <label className="field">Description detail<select value={p.character.description_detail} onChange={(e) => setC("description_detail", e.target.value)}><option>SPARSE</option><option>DETAILED</option></select></label>
          <label className="field">Motivation lock<select value={p.character.motivation_lock} onChange={(e) => setC("motivation_lock", e.target.value)}><option>FLEXIBLE</option><option>FIXED_FOR_TRIAL</option></select></label>
          <label className="field">Role rigidity<select value={p.character.role_rigidity} onChange={(e) => setC("role_rigidity", e.target.value)}><option>SOFT</option><option>FIRM</option><option>STRICT</option></select></label>
          <label className="field">Belief updates<select value={p.character.belief_update_policy} onChange={(e) => setC("belief_update_policy", e.target.value)}><option>UPDATE_ON_EVIDENCE</option><option>RESISTANT</option><option>LOCKED</option></select></label>
          <label className="field">Expressive style<input value={p.character.expressive_style} onChange={(e) => setC("expressive_style", e.target.value)} /></label>
          <label className="field">Output tokens per turn<input type="number" min={1} max={2000} value={p.limits.turn_output_tokens} onChange={(e) => set("limits", { ...p.limits, turn_output_tokens: Number(e.target.value) })} /></label>
        </div>
        <div className="grid3">
          {(["sociability", "skepticism", "expressivity"] as const).map((k) => (
            <label key={k} className="field">{k} ({p.character.social_axes[k].toFixed(2)}) — test knob, not a measurement
              <input type="range" min={0} max={1} step={0.05} value={p.character.social_axes[k]} onChange={(e) => setC("social_axes", { ...p.character.social_axes, [k]: Number(e.target.value) })} />
            </label>
          ))}
        </div>
        <h3>Allowed actions (explicit capability grants)</h3>
        <div className="row">{ALL_ACTIONS.map((a) => <label key={a} className="check"><input type="checkbox" checked={p.autonomy.allowed_actions.includes(a)} onChange={() => toggleAction(a)} />{a}</label>)}</div>
        {!editing && (
          <>
            <h3>Rooms</h3>
            <div className="row">{rooms.map((r) => <label key={r.id} className="check"><input type="checkbox" checked={roomIds.includes(r.id)} onChange={() => setRoomIds(roomIds.includes(r.id) ? roomIds.filter((x) => x !== r.id) : [...roomIds, r.id])} />{r.name}</label>)}</div>
          </>
        )}
        <div className="row" style={{ justifyContent: "flex-end" }}>
          <button className="btn" onClick={onClose}>Cancel</button>
          <button className="btn primary" onClick={save} disabled={!p.display_name.trim()}>{editing ? "Save changes" : "Create agent"}</button>
        </div>
      </div>
    </Modal>
  );
}
