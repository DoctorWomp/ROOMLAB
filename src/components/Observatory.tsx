import { useEffect, useMemo, useState } from "react";
import { api, errText } from "../lib/api";
import type { AgentSummary, EventEnvelope, RoomInfo, ViewRole } from "../lib/schemas";
import { useRun } from "./ui";

type Tab = "events" | "knowledge" | "manifest" | "playback" | "metrics" | "export" | "director";

function pct(v: number[], p: number) {
  if (!v.length) return null;
  const s = [...v].sort((a, b) => a - b);
  return s[Math.min(s.length - 1, Math.round(p * (s.length - 1)))];
}

export default function Observatory({ runId, rooms, agents, viewRole, tick }: { runId: string; rooms: RoomInfo[]; agents: AgentSummary[]; viewRole: ViewRole; tick: number }) {
  const run = useRun();
  const [tab, setTab] = useState<Tab>("events");
  const [events, setEvents] = useState<EventEnvelope[]>([]);
  const [types, setTypes] = useState<string>("");
  const [origin, setOrigin] = useState<string>("");
  const [roomF, setRoomF] = useState<string>("");
  const [open, setOpen] = useState<string | null>(null);
  const [matrix, setMatrix] = useState<Record<string, any>[] | null>(null);
  const [manifest, setManifest] = useState<Record<string, unknown> | null>(null);
  const [upto, setUpto] = useState(0);
  const [frame, setFrame] = useState<{ upto_seq: number; max_seq: number; events: EventEnvelope[]; note: string } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const op = viewRole === "OPERATOR";

  useEffect(() => {
    if (tab !== "events" && tab !== "metrics" && tab !== "director") return;
    api.listEvents(runId, { room_id: roomF || null, event_types: types ? [types] : [], origin_id: origin || null, after_seq: 0, upto_seq: null, limit: 5000 })
      .then((e) => { setEvents(e); setErr(null); }).catch((e) => setErr(errText(e)));
  }, [runId, tab, types, origin, roomF, tick]);
  useEffect(() => {
    if (tab === "knowledge" && op) api.knowledgeMatrix(runId).then(setMatrix).catch((e) => setErr(errText(e)));
    if (tab === "manifest") api.manifest(runId).then(setManifest).catch((e) => setErr(errText(e)));
  }, [runId, tab, op, tick]);
  useEffect(() => {
    if (tab !== "playback") return;
    api.playback(runId, upto || 1_000_000).then((f) => { setFrame(f); if (!upto) setUpto(f.max_seq); }).catch((e) => setErr(errText(e)));
  }, [runId, tab, upto]);

  const inference = useMemo(() => events.filter((e) => e.event_type === "INFERENCE_COMPLETED"), [events]);
  const allTypes = ["", "MESSAGE_DELIVERED", "ACTION_EXECUTED", "ACTION_REJECTED", "WORLD_STATE_CHANGED", "SPAWN_STATE_CHANGED", "JOB_QUEUED", "JOB_FINISHED", "INFERENCE_COMPLETED", "INFERENCE_FAILED", "TASK_UPDATED", "ORG_CHANGED", "INTERVENTION", "INTENT_SEALED", "INTENT_REVEALED", "CRUCIBLE_ROUND", "KNOWLEDGE_ADDED", "ACL_CHANGED", "TOOL_RESULT", "RUN_PAUSED", "RUN_RESUMED", "RUN_STEPPED", "VIEW_ROLE_CHANGED"];

  return (
    <div className="col" style={{ flex: 1, minHeight: 0 }}>
      <div className="room-head">
        <span style={{ fontSize: 18 }}>🔭</span><span className="title">Observatory</span>
        <div className="tabs" role="tablist">
          {(["events", "knowledge", "manifest", "playback", "metrics", "export", "director"] as Tab[]).map((t) => (
            <button key={t} role="tab" aria-selected={tab === t} className={`tab ${tab === t ? "sel" : ""}`} onClick={() => setTab(t)}>
              {t === "knowledge" ? "Who knows what" : t[0].toUpperCase() + t.slice(1)}
            </button>
          ))}
        </div>
        <span style={{ flex: 1 }} />
        {op ? <span className="badge amber">OPERATOR TRUTH</span> : <span className="badge cyan">Participant view (filtered)</span>}
      </div>
      {err && <div className="banner err">{err}</div>}
      <div className="panel" style={{ flex: 1 }}>
        {tab === "events" && (
          <>
            <div className="row" style={{ marginBottom: 8 }}>
              <select value={types} onChange={(e) => setTypes(e.target.value)} aria-label="Event type">{allTypes.map((t) => <option key={t} value={t}>{t || "all event types"}</option>)}</select>
              <select value={origin} onChange={(e) => setOrigin(e.target.value)} aria-label="Origin"><option value="">all origins</option>{agents.map((a) => <option key={a.id} value={a.id}>{a.display_name}</option>)}<option value="world">world</option><option value="scheduler">scheduler</option><option value="gateway">gateway</option><option value="spawn_governor">spawn governor</option></select>
              <select value={roomF} onChange={(e) => setRoomF(e.target.value)} aria-label="Room"><option value="">all rooms</option>{rooms.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}</select>
              <span className="faint small">{events.length} events · seq strictly increasing · causal parents linked</span>
            </div>
            <table className="t">
              <thead><tr><th>seq</th><th>type</th><th>origin</th><th>audience</th><th>summary</th></tr></thead>
              <tbody>
                {[...events].reverse().slice(0, 800).map((e) => (
                  <>
                    <tr key={e.event_id} onClick={() => setOpen(open === e.event_id ? null : e.event_id)} style={{ cursor: "pointer" }}>
                      <td className="mono">{e.seq}</td><td>{e.event_type}</td><td>{e.origin.kind.toLowerCase()}:{e.origin.id}</td>
                      <td><span className={`badge ${e.audience_scope === "OPERATOR_ONLY" ? "amber" : "outline"}`}>{e.audience_scope}</span></td>
                      <td className="muted small">{summarize(e)}</td>
                    </tr>
                    {open === e.event_id && <tr key={e.event_id + "x"}><td colSpan={5}><pre className="json">{JSON.stringify(e, null, 2)}</pre></td></tr>}
                  </>
                ))}
              </tbody>
            </table>
          </>
        )}
        {tab === "knowledge" && (op ? (
          <div className="col">
            <div className="banner truth" style={{ borderRadius: 6 }}>OPERATOR TRUTH — what each identity has actually observed and can read. Agents never see this view.</div>
            {(matrix ?? []).map((a) => (
              <div key={a.agent_id} className="card small">
                <div className="row"><b>{a.icon} {a.display_name}</b><span className="badge">{a.status}</span>{Object.entries(a.observation_counts).map(([k, n]) => <span key={k} className="badge outline">{k} {String(n)}</span>)}</div>
                {a.beliefs.length > 0 && <div>Beliefs: {a.beliefs.map((b: any) => `“${b.claim}” (${b.status}, ${b.source})`).join("; ")}</div>}
                <div>Readable: {a.accessible_resources.length ? a.accessible_resources.map((r: any) => r.title).join(", ") : <span className="faint">nothing beyond chat</span>}</div>
                {a.notable_observations.length > 0 && <div className="muted">Notable: {a.notable_observations.map((o: any) => `#${o.seq} ${o.kind} ${JSON.stringify(o.data).slice(0, 120)}`).join(" · ")}</div>}
              </div>
            ))}
          </div>
        ) : <div className="empty">Who-knows-what is operator-only. Switch to Operator view (in a controlled trial this marks the run contaminated).</div>)}
        {tab === "manifest" && manifest && <pre className="json" style={{ maxHeight: "none" }}>{JSON.stringify(manifest, null, 2)}</pre>}
        {tab === "playback" && frame && (
          <div className="col">
            <div className="banner playback" style={{ borderRadius: 6 }}>{frame.note}</div>
            <label className="field">Replay up to event #{upto} of {frame.max_seq}
              <input type="range" min={1} max={frame.max_seq} value={upto} onChange={(e) => setUpto(Number(e.target.value))} />
            </label>
            <table className="t"><thead><tr><th>seq</th><th>type</th><th>origin</th><th>summary</th></tr></thead>
              <tbody>{[...frame.events].reverse().slice(0, 300).map((e) => <tr key={e.event_id}><td className="mono">{e.seq}</td><td>{e.event_type}</td><td>{e.origin.id}</td><td className="muted small">{summarize(e)}</td></tr>)}</tbody></table>
          </div>
        )}
        {tab === "metrics" && (
          <div className="col">
            <div className="small muted">Measured from real provider calls in this run only (no estimates shown as measurements). {inference.length < 20 && "Fewer than 20 samples: p95 is indicative only."}</div>
            <Metrics events={inference} />
          </div>
        )}
        {tab === "export" && (
          <div className="col" style={{ maxWidth: 680 }}>
            <div className="card col">
              <b>Export this run</b>
              <div className="small muted">Exports never contain provider keys. Public exports omit private messages, operator-only events, private knowledge and sealed answer keys. A JSONL event file is written alongside.</div>
              <div className="row">
                <button className="btn" onClick={() => run(async () => `Saved: ${await api.exportRun(runId, "PUBLIC")}`).then((m) => m && alert(m))}>Export public</button>
                <button className="btn" disabled={!op} onClick={() => run(async () => `Saved: ${await api.exportRun(runId, "OPERATOR")}`).then((m) => m && alert(m))}>Export operator (truth)</button>
                <button className="btn ghost" onClick={() => api.openFolder("exports")}>Open exports folder</button>
              </div>
            </div>
            <div className="card col">
              <b>Import a run (read-only playback)</b>
              <input type="file" accept=".json,application/json" onChange={async (e) => { const f = e.target.files?.[0]; if (!f) return; const t = await f.text(); const id = await run(() => api.importRun(t), "Imported as a PLAYBACK run — find it in the run menu"); if (id) e.target.value = ""; }} />
            </div>
          </div>
        )}
        {tab === "director" && <Director runId={runId} rooms={rooms} op={op} events={events} />}
      </div>
    </div>
  );
}

function summarize(e: EventEnvelope): string {
  const p = e.payload as Record<string, any>;
  switch (e.event_type) {
    case "ACTION_EXECUTED": return `${p.request?.action ?? p.action ?? ""} ${p.detail ? (typeof p.detail === "string" ? p.detail : JSON.stringify(p.detail)) : ""}`;
    case "ACTION_REJECTED": return `${p.request?.action}: ${p.reason_code} — ${p.reason}`;
    case "SPAWN_STATE_CHANGED": return `${p.request_id}: ${p.from ?? "∅"} → ${p.to} (${p.reason_code}) ${p.detail}`;
    case "JOB_QUEUED": return `${p.agent_id} · ${p.reason} · prio ${p.priority} · depth ${p.chain_depth}`;
    case "INFERENCE_COMPLETED": return `${p.agent_id} · ${p.resolved_model} · first token ${p.first_token_ms ?? "?"} ms · total ${p.total_ms} ms · ${p.parse}`;
    case "WORLD_STATE_CHANGED": return p.check ? `${p.check} check: ${p.observer ?? p.searcher} roll ${p.roll}+${p.bonus} vs ${p.dc} → ${p.success ? "success" : "fail"}` : JSON.stringify(p).slice(0, 140);
    default: return JSON.stringify(p).slice(0, 140);
  }
}

function Metrics({ events }: { events: EventEnvelope[] }) {
  const ft = events.map((e) => (e.payload as any).first_token_ms).filter((x: unknown) => typeof x === "number") as number[];
  const tot = events.map((e) => (e.payload as any).total_ms).filter((x: unknown) => typeof x === "number") as number[];
  const out = events.map((e) => (e.payload as any).usage?.completion_tokens).filter((x: unknown) => typeof x === "number") as number[];
  const f = (v: number | null) => (v == null ? "n/a" : String(v));
  return (
    <>
      <table className="t" style={{ maxWidth: 680 }}>
        <thead><tr><th>metric</th><th>n</th><th>p50</th><th>p95</th></tr></thead>
        <tbody>
          <tr><td>first visible token (ms)</td><td>{ft.length}</td><td>{f(pct(ft, 0.5))}</td><td>{f(pct(ft, 0.95))}</td></tr>
          <tr><td>total call (ms)</td><td>{tot.length}</td><td>{f(pct(tot, 0.5))}</td><td>{f(pct(tot, 0.95))}</td></tr>
          <tr><td>completion tokens (provider-reported)</td><td>{out.length}</td><td>{f(pct(out, 0.5))}</td><td>{f(pct(out, 0.95))}</td></tr>
        </tbody>
      </table>
      <div className="small faint">Cache hit rate: not measured. RAM/VRAM: not observable from RoomLab for external providers.</div>
    </>
  );
}

function Director({ runId, rooms, op, events }: { runId: string; rooms: RoomInfo[]; op: boolean; events: EventEnvelope[] }) {
  const run = useRun();
  const story = rooms.filter((r) => r.kind === "STORY");
  const [room, setRoom] = useState(story[0]?.id ?? "");
  const [entity, setEntity] = useState("lantern");
  const [loc, setLoc] = useState("cellar");
  const [state, setState] = useState("UNLOCKED");
  const [turns, setTurns] = useState(400);
  const [pre, setPre] = useState(false);
  const interventions = events.filter((e) => e.event_type === "INTERVENTION");
  if (!op) return <div className="empty">Director interventions require Operator view.</div>;
  return (
    <div className="col" style={{ maxWidth: 760 }}>
      <div className="small muted">Every Director edit is journaled with before/after state. In a controlled trial it marks the run contaminated unless pre-authorized.</div>
      <label className="check"><input type="checkbox" checked={pre} onChange={(e) => setPre(e.target.checked)} /> Pre-authorized treatment (does not contaminate)</label>
      {story.length > 0 && (
        <div className="card col">
          <b>World edit</b>
          <div className="row">
            <select value={room} onChange={(e) => setRoom(e.target.value)}>{story.map((r) => <option key={r.id} value={r.id}>{r.name}</option>)}</select>
            <input value={entity} onChange={(e) => setEntity(e.target.value)} placeholder="entity id" />
            <input value={loc} onChange={(e) => setLoc(e.target.value)} placeholder="location id" />
            <button className="btn sm" onClick={() => run(() => api.directorEdit(runId, { type: "MOVE_ENTITY", room_id: room, entity_id: entity, to_location: loc }, `move ${entity} → ${loc}`, pre), "Journaled")}>Move entity</button>
          </div>
          <div className="row">
            <input value={state} onChange={(e) => setState(e.target.value)} placeholder="state" />
            <button className="btn sm" onClick={() => run(() => api.directorEdit(runId, { type: "SET_ENTITY_STATE", room_id: room, entity_id: entity, state }, `set ${entity}.state=${state}`, pre), "Journaled")}>Set state</button>
          </div>
        </div>
      )}
      <div className="card row">
        <b>Run turn budget</b><input type="number" value={turns} onChange={(e) => setTurns(Number(e.target.value))} />
        <button className="btn sm" onClick={() => run(() => api.directorEdit(runId, { type: "SET_BUDGET", output_tokens: null, agent_turns: turns }, `budget turns=${turns}`, pre), "Journaled")}>Set</button>
        <button className="btn sm ghost" onClick={() => run(() => api.runControl(runId, "clear_queue"), "Queue cleared")}>Clear queued turns</button>
      </div>
      <h3>Interventions ({interventions.length})</h3>
      {interventions.map((e) => <div key={e.event_id} className="card small"><b>#{e.seq}</b> {(e.payload as any).description} {(e.payload as any).preauthorized ? <span className="badge green">pre-authorized</span> : <span className="badge amber">unplanned</span>}<pre className="json">{JSON.stringify({ before: (e.payload as any).before, after: (e.payload as any).after }, null, 2)}</pre></div>)}
    </div>
  );
}
