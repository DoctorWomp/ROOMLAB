import { useEffect, useMemo, useState } from "react";
import { api, errText } from "../lib/api";
import type { MemberInfo, OrgEdge, RoomInfo, SpawnRow, TaskCluster } from "../lib/schemas";
import { useRun } from "./ui";

const REL_COLOR: Record<string, string> = { reports_to: "#8b5cf6", assigned_task: "#22d3ee", reviewed_by: "#f59e0b", can_delegate: "#34d399", can_nominate: "#f472b6" };

export default function OrgChart({ runId, room, spawns, tick }: { runId: string; room: RoomInfo; spawns: SpawnRow[]; tick: number }) {
  const run = useRun();
  const [g, setG] = useState<{ edges: OrgEdge[]; members: MemberInfo[]; tasks: TaskCluster[]; mode: string } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [rel, setRel] = useState("reports_to");
  const [goal, setGoal] = useState("");
  const [assignees, setAssignees] = useState<string[]>([]);

  useEffect(() => { api.orgGraph(runId, room.id).then(setG).catch((e) => setErr(errText(e))); }, [runId, room.id, tick]);

  const layout = useMemo(() => {
    if (!g) return { pos: new Map<string, [number, number]>(), nodes: [] as MemberInfo[] };
    const nodes = g.members.filter((m) => m.status !== "ARCHIVED" || m.parent_agent_id);
    // Level: human 0; nodes reporting to human 1; others 2; children 3.
    const level = (m: MemberInfo) => (m.kind === "HUMAN" ? 0 : m.parent_agent_id ? 3 : g.edges.some((e) => e.from_id === m.agent_id && e.relation === "reports_to" && e.to_id === "human_01") ? 1 : 2);
    const rows: MemberInfo[][] = [[], [], [], []];
    nodes.forEach((m) => rows[level(m)].push(m));
    const pos = new Map<string, [number, number]>();
    rows.forEach((r, li) => r.forEach((m, i) => pos.set(m.agent_id, [((i + 1) * 520) / (r.length + 1), 34 + li * 78])));
    return { pos, nodes };
  }, [g]);

  if (err) return <div className="err small">{err}</div>;
  if (!g) return <div className="faint">Loading…</div>;
  const name = (id: string) => g.members.find((m) => m.agent_id === id)?.display_name ?? id;
  const roomSpawns = spawns.filter((s) => s.room_id === room.id && s.spawn_type === "TASK_WORKER");
  const coordinator = g.members.find((m) => m.role === "coordinator");

  return (
    <div className="col org">
      <h2>Org graph <span className="badge">{g.mode}</span></h2>
      <div className="small faint">Org edges route work only. They never grant knowledge access, tools or spawn rights.</div>
      <svg viewBox="0 0 520 300" role="img" aria-label="Organization chart">
        <defs>{Object.entries(REL_COLOR).map(([k, c]) => <marker key={k} id={`a-${k}`} viewBox="0 0 10 10" refX="9" refY="5" markerWidth="6" markerHeight="6" orient="auto-start-reverse"><path d="M0,0 L10,5 L0,10 z" fill={c} /></marker>)}</defs>
        {g.edges.map((e) => {
          const a = layout.pos.get(e.from_id), b = layout.pos.get(e.to_id);
          if (!a || !b) return null;
          const mx = (a[0] + b[0]) / 2 + (e.relation === "reports_to" ? 0 : 18);
          return <path key={e.id} d={`M${a[0]},${a[1]} Q${mx},${(a[1] + b[1]) / 2} ${b[0]},${b[1] + (b[1] < a[1] ? 14 : -14)}`} stroke={REL_COLOR[e.relation] ?? "#888"} markerEnd={`url(#a-${e.relation})`} opacity={0.85} />;
        })}
        {layout.nodes.map((m) => {
          const [x, y] = layout.pos.get(m.agent_id)!;
          return (
            <g key={m.agent_id} className="node" transform={`translate(${x - 52},${y - 14})`} opacity={m.status === "ARCHIVED" ? 0.45 : 1}>
              <rect width="104" height="28" rx="8" />
              <text x="52" y="18" textAnchor="middle">{m.icon} {m.display_name}</text>
            </g>
          );
        })}
      </svg>
      <div className="row small">{Object.entries(REL_COLOR).map(([k, c]) => <span key={k} className="row" style={{ gap: 4 }}><span className="dot" style={{ background: c }} />{k}</span>)}</div>
      <div className="row">
        <select value={from} onChange={(e) => setFrom(e.target.value)} aria-label="From"><option value="">from…</option>{g.members.map((m) => <option key={m.agent_id} value={m.agent_id}>{m.display_name}</option>)}</select>
        <select value={rel} onChange={(e) => setRel(e.target.value)} aria-label="Relation">{Object.keys(REL_COLOR).map((r) => <option key={r}>{r}</option>)}</select>
        <select value={to} onChange={(e) => setTo(e.target.value)} aria-label="To"><option value="">to…</option>{g.members.map((m) => <option key={m.agent_id} value={m.agent_id}>{m.display_name}</option>)}</select>
        <button className="btn sm" disabled={!from || !to} onClick={() => run(() => api.orgEdit(runId, room.id, [from, to, rel], null), "Edge added")}>Add edge</button>
      </div>
      <details><summary className="small">Remove edges ({g.edges.length})</summary>
        {g.edges.map((e) => <div key={e.id} className="row small"><span className="grow">{name(e.from_id)} —{e.relation}→ {name(e.to_id)}</span><button className="btn ghost sm" onClick={() => run(() => api.orgEdit(runId, room.id, null, e.id), "Edge removed")} aria-label="Remove edge">✕</button></div>)}
      </details>

      <h2>Task clusters</h2>
      {g.tasks.length === 0 && <div className="small faint">No tasks yet. Post a request in the chat (the coordinator will assign it) or create one here.</div>}
      {g.tasks.map((t) => (
        <div key={t.cluster_id} className="card small">
          <div className="row"><b>{t.cluster_id}</b><span className={`badge ${t.status === "VERIFIED" ? "green" : t.status === "READY_FOR_REVIEW" ? "amber" : "cyan"}`}>{t.status}</span>{t.human_action_needed && <span className="badge amber">human action needed</span>}</div>
          <div>{t.goal}</div>
          <div className="faint">owner {name(t.owner_agent_id)} · assignees {t.assignees.map(name).join(", ")}</div>
          <ul style={{ margin: "4px 0", paddingLeft: 18 }}>{t.verification_criteria.map((c) => <li key={c}>{c}</li>)}</ul>
          {t.notes.map((n, i) => <div key={i} className="faint">• {name(n.by)}: {n.note}</div>)}
          <div className="row" style={{ marginTop: 6 }}>
            {["IN_PROGRESS", "BLOCKED", "READY_FOR_REVIEW", "VERIFIED", "ABANDONED"].map((s) => (
              <button key={s} className={`btn sm ${s === "VERIFIED" ? "primary" : ""}`} disabled={t.status === s} onClick={() => run(() => api.humanAction(runId, room.id, "UPDATE_TASK", { cluster_id: t.cluster_id, status: s }), `Marked ${s}`)}>{s.replace(/_/g, " ").toLowerCase()}</button>
            ))}
          </div>
        </div>
      ))}
      <div className="card col">
        <b className="small">New task cluster</b>
        <input placeholder="Goal" value={goal} onChange={(e) => setGoal(e.target.value)} />
        <div className="row small">{g.members.filter((m) => m.kind === "AGENT" && !m.parent_agent_id).map((m) => <label key={m.agent_id} className="check"><input type="checkbox" checked={assignees.includes(m.agent_id)} onChange={() => setAssignees(assignees.includes(m.agent_id) ? assignees.filter((x) => x !== m.agent_id) : [...assignees, m.agent_id])} />{m.display_name}</label>)}</div>
        <button className="btn sm primary" disabled={!goal.trim() || !assignees.length} onClick={async () => { if (await run(() => api.humanAction(runId, room.id, "ASSIGN_TASK", { goal, assignees }), "Task created")) { setGoal(""); setAssignees([]); } }}>Create & brief assignees</button>
      </div>

      <h2>Sub-agents</h2>
      <div className="small faint">One-level task workers. Requests need operator approval (default); caps: {room.settings.max_children_per_parent}/parent, {room.settings.max_active_children}/room, depth 1, TTL {room.settings.child_ttl_seconds}s.</div>
      {roomSpawns.map((s) => (
        <div key={s.request_id} className="card small">
          <div className="row"><b>{s.display_name}</b><span className={`badge ${s.state === "ACTIVE" ? "cyan" : s.state === "AWAIT_APPROVAL" ? "amber" : s.state === "ARCHIVED" ? "green" : s.state === "REJECTED" ? "red" : ""}`}>{s.state}</span><span className="faint">by {s.parent_name}</span></div>
          <div className="muted">{s.objective}</div>
          <div className="faint">{s.turns_used != null ? `turns ${s.turns_used}/${s.max_turns}` : ""} {s.end_reason ? `· ${s.end_reason}` : ""} {s.reason_code !== "OK" ? `· ${s.reason_code}` : ""}</div>
          {s.state === "AWAIT_APPROVAL" && <div className="row"><button className="btn sm primary" onClick={() => run(() => api.decideSpawn(runId, s.request_id, true), "Approved")}>Approve</button><button className="btn sm danger" onClick={() => run(() => api.decideSpawn(runId, s.request_id, false), "Denied")}>Deny</button></div>}
          {s.state === "ACTIVE" && s.child_agent_id && <button className="btn sm danger" onClick={() => run(() => api.stopChild(runId, s.child_agent_id!), "Stopped")}>Stop</button>}
        </div>
      ))}
      {coordinator && (
        <button className="btn sm" onClick={() => run(() => api.operatorSpawn(runId, room.id, coordinator.agent_id, "Extra Worker", "Double-check the leading explanation against the logs."), "Requested (caps still apply)")} title="Operator-initiated child for the coordinator; caps and queueing still apply">
          ＋ Request extra worker for {coordinator.display_name}
        </button>
      )}
    </div>
  );
}
