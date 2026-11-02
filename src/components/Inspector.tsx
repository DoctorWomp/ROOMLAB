import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import type { AgentProfile, CompiledPrompt, RoomInfo, ViewRole } from "../lib/schemas";
import { CognitionBadge, StatusDot, useRun } from "./ui";

type Insp = Record<string, any> & { profile: AgentProfile; prompt_preview?: CompiledPrompt | null };

export default function Inspector(p: { runId: string; agentId: string; rooms: RoomInfo[]; roomId: string | null; viewRole: ViewRole; tick: number; onClose: () => void; onEdit: (id: string) => void }) {
  const run = useRun();
  const [d, setD] = useState<Insp | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [tab, setTab] = useState<"profile" | "knowledge" | "history" | "spawns" | "prompt">("profile");
  const [q, setQ] = useState("");
  const [hits, setHits] = useState<Record<string, any>[] | null>(null);
  const [archive, setArchive] = useState<Record<string, any> | null>(null);
  const op = p.viewRole === "OPERATOR";

  useEffect(() => {
    api.agentInspector(p.runId, p.agentId).then((x) => { setD(x as Insp); setErr(null); }).catch((e) => setErr(errText(e)));
  }, [p.runId, p.agentId, p.tick]);

  if (err) return <div className="err small">{err} <button className="btn sm" onClick={p.onClose}>Close</button></div>;
  if (!d) return <div className="faint">Loading…</div>;
  const s = d.summary ?? {};
  const prof = d.profile;
  const human = s.kind === "HUMAN";

  return (
    <div className="col">
      <div className="row">
        <span style={{ fontSize: 26 }}>{prof.icon}</span>
        <div className="grow">
          <div style={{ fontWeight: 700 }}>{prof.display_name} <span className="faint small mono">{prof.agent_id}</span></div>
          <div className="small muted">{prof.character.role}</div>
        </div>
        <button className="btn ghost" onClick={p.onClose} aria-label="Close inspector">✕</button>
      </div>
      <div className="row small">
        <StatusDot status={s.status ?? "ACTIVE"} /> {s.status}
        {!human && <CognitionBadge c={prof.cognition} route={prof.model_route} />}
        <span className="badge">route: {prof.model_route}</span>
        <span className="badge violet">SOMA {prof.soma_mode}</span>
        {s.parent_agent_id && <span className="badge outline">child of {s.parent_agent_id}</span>}
      </div>
      <div className="small muted">queued {s.queued_jobs ?? 0} · active {s.running_jobs ?? 0} · model calls {s.calls ?? 0} · output tokens {s.output_tokens ?? 0} · scripted turns {s.demo_turns ?? 0}</div>
      {!human && (
        <div className="row">
          <button className="btn sm" onClick={() => p.onEdit(prof.agent_id)}>✎ Edit</button>
          {s.status === "ACTIVE" && <button className="btn sm" onClick={() => run(() => api.setAgentStatus(p.runId, prof.agent_id, "DORMANT"), "Now dormant (no compute; still observes)")}>Suspend</button>}
          {s.status === "DORMANT" && <button className="btn sm cyan" onClick={() => { const qq = window.prompt("Wake with a question (optional):", ""); if (qq !== null && p.roomId) run(() => api.wakeAgent(p.runId, prof.agent_id, p.roomId!, qq), "Woken"); }}>Wake</button>}
          {s.parent_agent_id && s.status === "ACTIVE" && <button className="btn sm danger" onClick={() => run(() => api.stopChild(p.runId, prof.agent_id), "Child stopped and archived")}>Stop child</button>}
          {s.status !== "ARCHIVED" && <button className="btn sm danger" onClick={() => window.confirm(`Delete (archive) ${prof.display_name}? History is kept.`) && run(() => api.deleteAgent(p.runId, prof.agent_id), "Archived")}>Delete</button>}
        </div>
      )}
      <div className="tabs" role="tablist">
        {(["profile", "knowledge", "history", "spawns", "prompt"] as const).map((t) => (
          <button key={t} role="tab" aria-selected={tab === t} className={`tab ${tab === t ? "sel" : ""}`} onClick={() => setTab(t)}>{t[0].toUpperCase() + t.slice(1)}</button>
        ))}
      </div>
      {tab === "profile" && (
        <div className="col small">
          <div><b>Persona</b><div className="muted">{prof.persona || "—"}</div></div>
          <div><b>Directive</b><div className="muted">{prof.directive || "—"}</div></div>
          <div className="grid2">
            <div>Motivation: <span className="muted">{prof.character.motivation_core || "—"} ({prof.character.motivation_lock})</span></div>
            <div>Rigidity: <span className="muted">{prof.character.role_rigidity}</span></div>
            <div>Detail: <span className="muted">{prof.character.description_detail}</span></div>
            <div>Beliefs: <span className="muted">{prof.character.belief_update_policy}</span></div>
          </div>
          <div>Allowed actions <span className="faint">(explicit grants; org rank grants nothing)</span>: <div className="row" style={{ gap: 4, marginTop: 4 }}>{prof.autonomy.allowed_actions.map((a) => <span key={a} className="badge outline">{a}</span>)}</div></div>
          <div>Rooms: <span className="muted">{(d.rooms as string[]).map((r) => p.rooms.find((x) => x.id === r)?.name ?? r).join(", ")}</span></div>
          <div className="faint">Profile version {d.profile_version}. Social axes are test knobs, not measurements.</div>
        </div>
      )}
      {tab === "knowledge" && (
        op ? (
          <div className="col small">
            <div className="badge amber">OPERATOR TRUTH — resources this identity is authorized to read</div>
            {(d.resources as any[]).length === 0 && <div className="faint">No readable resources.</div>}
            {(d.resources as any[]).map((r) => <div key={r.id} className="card"><b>{r.title}</b> <span className="faint mono">{r.id}</span><div className="faint">{r.namespace} · {r.kind} · v{r.version}</div></div>)}
            <h3>Search as this agent (ACL test)</h3>
            <div className="row"><input className="grow" value={q} onChange={(e) => setQ(e.target.value)} placeholder="query" /><button className="btn sm" onClick={async () => setHits((await run(() => api.knowledgeSearchAs(p.runId, prof.agent_id, q))) ?? null)}>Search</button></div>
            {hits && (hits.length ? hits.map((h) => <div key={h.id} className="card"><b>{h.title}</b> <span className="faint">{h.namespace}</span><div className="muted">{h.snippet}</div></div>) : <div className="faint">No authorized hits (unauthorized titles are not revealed).</div>)}
            <h3>Archive lookup (no inference)</h3>
            <button className="btn sm" onClick={async () => { const qq = window.prompt("Archive question:", ""); if (qq) setArchive((await run(() => api.lookupArchive(p.runId, prof.agent_id, qq))) ?? null); }}>LOOKUP_ARCHIVE…</button>
            {archive && <pre className="json">{JSON.stringify(archive, null, 2)}</pre>}
          </div>
        ) : <div className="faint small">Knowledge inspection requires Operator view.</div>
      )}
      {tab === "history" && (
        <div className="col small">
          <div><b>Turn outcomes</b> <span className="faint">(offered vs suppressed vs silence are logged separately)</span></div>
          <div className="row">{(d.turn_outcomes as any[]).map((t) => <span key={t.outcome} className="badge outline">{t.outcome}: {t.count}</span>)}</div>
          <table className="t"><thead><tr><th>seq</th><th>status</th><th>reason</th><th>depth</th><th>outcome</th></tr></thead>
            <tbody>{(d.recent_jobs as any[]).map((j) => <tr key={j.job_id}><td>{j.seq}</td><td>{j.status}</td><td>{j.reason}</td><td>{j.chain_depth}</td><td className="muted">{j.outcome}</td></tr>)}</tbody></table>
        </div>
      )}
      {tab === "spawns" && (
        <div className="col small">
          {(d.children as any[]).length === 0 && <div className="faint">No children.</div>}
          {(d.children as any[]).map((c) => <div key={c.request_id} className="card"><b>{c.display_name}</b> <span className="badge">{c.state}</span><div className="muted">{c.objective}</div><div className="faint">turns {c.turns_used ?? 0}/{c.max_turns ?? "?"} {c.end_reason ? `· ended: ${c.end_reason}` : ""}</div></div>)}
        </div>
      )}
      {tab === "prompt" && (
        op && d.prompt_preview ? (
          <div className="col small">
            <div className="badge amber">OPERATOR TRUTH — the exact context RoomLab would assemble for this identity</div>
            <div className="faint">~{d.prompt_preview.token_estimate} tokens (estimate) · {d.prompt_preview.observation_count} delivered observations · template {d.prompt_preview.template_hash}</div>
            {d.prompt_preview.segments.map((sg, i) => <details key={i} open={i >= 3}><summary><b>{sg.label}</b></summary><pre className="json">{sg.text}</pre></details>)}
          </div>
        ) : <div className="faint small">{op ? "No prompt preview (not in a room)." : "Prompt inspection requires Operator view."}</div>
      )}
    </div>
  );
}
