import { useState } from "react";
import { api } from "../lib/api";
import type { AgentSummary, DraftUpdate, RoomInfo, RoomKind, SpawnRow } from "../lib/schemas";
import { CognitionBadge, roomIcon, StatusDot, useRun } from "./ui";

export default function RoomNavigator(p: {
  runId: string; rooms: RoomInfo[]; roomId: string | null; agents: AgentSummary[]; spawns: SpawnRow[]; drafts: Record<string, DraftUpdate>;
  onRoom: (id: string) => void; onInspect: (id: string) => void; onNewAgent: () => void;
}) {
  const run = useRun();
  const [adding, setAdding] = useState(false);
  const [kind, setKind] = useState<RoomKind>("STORY");
  const [name, setName] = useState("");
  const room = p.rooms.find((r) => r.id === p.roomId);
  const drafting = new Set(Object.values(p.drafts).map((d) => d.agent_id));
  const agentById = new Map(p.agents.map((a) => [a.id, a]));
  const top = room?.members.filter((m) => !m.parent_agent_id) ?? [];
  const children = (pid: string) => room?.members.filter((m) => m.parent_agent_id === pid) ?? [];

  const create = async () => {
    const id = await run(() => api.createRoom(p.runId, kind, name.trim() || kind.toLowerCase()), "Room created");
    if (id) {
      setAdding(false);
      setName("");
      p.onRoom(id);
    }
  };

  return (
    <>
      <div className="nav-section">
        <div className="nav-title"><span>Rooms</span><button className="btn ghost sm" onClick={() => setAdding(!adding)} aria-label="New room">＋</button></div>
        {adding && (
          <div className="card col" style={{ marginBottom: 8 }}>
            <select value={kind} onChange={(e) => setKind(e.target.value as RoomKind)} aria-label="Room type">
              <option value="COMMONS">💬 Commons</option><option value="GUILD">🧩 Guild</option><option value="STORY">📜 Story</option><option value="CRUCIBLE">🧪 Crucible</option>
            </select>
            <input placeholder="Room name" value={name} onChange={(e) => setName(e.target.value)} onKeyDown={(e) => e.key === "Enter" && create()} />
            <button className="btn primary sm" onClick={create}>Create from template</button>
          </div>
        )}
        {p.rooms.map((r) => (
          <button key={r.id} className={`nav-item ${r.id === p.roomId ? "sel" : ""}`} onClick={() => p.onRoom(r.id)} aria-current={r.id === p.roomId}>
            <span className="icon">{roomIcon(r.kind)}</span>
            <span className="grow" style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{r.name}</span>
            <span className="meta">{r.members.length}</span>
          </button>
        ))}
      </div>
      {room && (
        <div className="nav-section">
          <div className="nav-title"><span>Members</span><button className="btn ghost sm" onClick={p.onNewAgent} aria-label="Create agent">＋ agent</button></div>
          {top.map((m) => {
            const a = agentById.get(m.agent_id);
            return (
              <div key={m.agent_id}>
                <button className="nav-item member" onClick={() => p.onInspect(m.agent_id)} title={`${m.role} · ${m.cognition}`}>
                  <StatusDot status={m.status} />
                  <span className="icon">{m.icon}</span>
                  <span className="grow" style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{m.display_name}</span>
                  {drafting.has(m.agent_id) ? <span className="badge cyan pulse">drafting</span> : a && a.running_jobs > 0 ? <span className="badge cyan">active</span> : a && a.queued_jobs > 0 ? <span className="badge">queued</span> : null}
                  {m.kind === "HUMAN" ? <span className="badge outline">you</span> : m.kind === "NARRATOR" ? <span className="badge violet">narr.</span> : <CognitionBadge c={m.cognition} route={m.model_route} />}
                </button>
                {children(m.agent_id).length > 0 && <div className="faint small" style={{ paddingLeft: 26 }}>▾ sub-agents</div>}
                {children(m.agent_id).map((c) => (
                  <button key={c.agent_id} className="nav-item member child" onClick={() => p.onInspect(c.agent_id)}>
                    <StatusDot status={c.status} /><span className="icon">{c.icon}</span><span className="grow">{c.display_name}</span>
                    <span className="badge outline">{c.status === "ARCHIVED" ? "archived" : "child"}</span>
                  </button>
                ))}
              </div>
            );
          })}
        </div>
      )}
      {p.spawns.some((s) => s.state === "AWAIT_APPROVAL") && (
        <div className="nav-section">
          <div className="nav-title"><span>Spawn approvals</span></div>
          {p.spawns.filter((s) => s.state === "AWAIT_APPROVAL").map((s) => (
            <div key={s.request_id} className="card small">
              <div><b>{s.parent_name}</b> requests <b>{s.display_name}</b></div>
              <div className="muted">{s.objective}</div>
              <div className="row" style={{ marginTop: 6 }}>
                <button className="btn primary sm" onClick={() => run(() => api.decideSpawn(p.runId, s.request_id, true), "Approved")}>Approve</button>
                <button className="btn danger sm" onClick={() => run(() => api.decideSpawn(p.runId, s.request_id, false), "Denied")}>Deny</button>
              </div>
            </div>
          ))}
        </div>
      )}
    </>
  );
}
