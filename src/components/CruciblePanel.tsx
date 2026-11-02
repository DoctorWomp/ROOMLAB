import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import type { IntentCard, RoomInfo, ViewRole } from "../lib/schemas";
import { useRun } from "./ui";

const emptyCard: IntentCard = { intended_stance: "", intended_lane: "", load_bearing_cue: "", predicted_misread: "", what_counts_as_hit: "" };

export default function CruciblePanel({ runId, room, tick, viewRole }: { runId: string; room: RoomInfo; tick: number; viewRole: ViewRole }) {
  const run = useRun();
  const [data, setData] = useState<{ cases: Record<string, any>[]; envelopes: Record<string, any>[] } | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [caseText, setCaseText] = useState("");
  const [rounds, setRounds] = useState(1);
  const [card, setCard] = useState<IntentCard>(emptyCard);
  const [signal, setSignal] = useState("");
  const [laneVisible, setLaneVisible] = useState(false);

  useEffect(() => { api.crucibleCases(runId, room.id).then(setData).catch((e) => setErr(errText(e))); }, [runId, room.id, tick]);
  if (err) return <div className="err small">{err}</div>;
  if (!data) return <div className="faint">Loading…</div>;
  const roles = ["steelman", "skeptic", "mediator"].map((r) => room.members.find((m) => m.role === r));

  return (
    <div className="col">
      <h2>Crucible</h2>
      <div className="row small">{roles.map((m, i) => m ? <span key={i} className="badge violet">{m.icon} {m.display_name} · {m.role}</span> : null)}<span className="badge outline">evaluator: you (outside the room's prompts)</span></div>
      <div className="card col">
        <b className="small">New case</b>
        <textarea rows={3} placeholder="Contentious proposal or interpretation to examine…" value={caseText} onChange={(e) => setCaseText(e.target.value)} />
        <div className="row"><label className="field">Rounds<select value={rounds} onChange={(e) => setRounds(Number(e.target.value))}>{[1, 2, 3].map((n) => <option key={n}>{n}</option>)}</select></label>
          <span style={{ flex: 1 }} />
          <button className="btn primary" disabled={!caseText.trim()} onClick={async () => { if (await run(() => api.crucibleStart(runId, room.id, caseText, rounds), "Case started: steelman → skeptic → mediator")) setCaseText(""); }}>Start review</button></div>
      </div>
      <h3>Cases</h3>
      {data.cases.length === 0 && <div className="small faint">No cases yet.</div>}
      {data.cases.map((c) => (
        <div key={c.case_id} className="card small">
          <div className="row"><b className="grow">{c.case_text}</b><span className="badge">{c.rounds} round(s)</span>
            <button className="btn sm" onClick={() => run(async () => `Exported to ${await api.crucibleExport(runId, c.case_id)}`).then((m) => m && alert(m))}>Export record</button></div>
        </div>
      ))}
      <h3>Sealed intent envelope (blind interpretation test)</h3>
      <div className="small faint">The intent card is sealed in evaluator-only storage <i>before</i> receivers see the signal. Receivers cannot retrieve it through prompts or search until you reveal it.</div>
      <div className="card col">
        <div className="grid2">
          {(Object.keys(emptyCard) as (keyof IntentCard)[]).map((k) => <label key={k} className="field">{k.replace(/_/g, " ")}<input value={card[k]} onChange={(e) => setCard({ ...card, [k]: e.target.value })} /></label>)}
        </div>
        <label className="field">Signal sent to the room<input value={signal} onChange={(e) => setSignal(e.target.value)} placeholder="You shredded my favorite theory. ... Please don't stop." /></label>
        <label className="check"><input type="checkbox" checked={laneVisible} onChange={(e) => setLaneVisible(e.target.checked)} /> Treatment arm: sender lane visible (otherwise withheld)</label>
        <button className="btn primary" disabled={!signal.trim() || !card.intended_stance.trim()} onClick={async () => {
          const steps = card.intended_lane.split(/→|,|>/).map((x) => x.trim()).filter(Boolean);
          const lane = laneVisible && steps.length >= 2 ? { owner: "SOMA_EXPRESSION" as const, owner_name: null, steps } : null;
          if (await run(() => api.intentSeal(runId, room.id, card, signal, laneVisible, lane), "Sealed; receivers are responding blind")) { setCard(emptyCard); setSignal(""); }
        }}>Seal & send</button>
      </div>
      {data.envelopes.map((e) => (
        <div key={e.id} className="card small">
          <div className="row"><b className="mono">{e.id}</b><span className={`badge ${e.sealed ? "amber" : "green"}`}>{e.sealed ? "SEALED" : "REVEALED"}</span>
            {e.sealed && viewRole === "OPERATOR" && <button className="btn sm primary" onClick={() => run(() => api.intentReveal(runId, e.id), "Revealed: receivers will reconcile")}>Reveal (stage C)</button>}</div>
          {e.card ? <pre className="json">{JSON.stringify(e.card, null, 2)}</pre> : <div className="faint">Card hidden in this view.</div>}
        </div>
      ))}
      <div className="small faint">Roles and confidence labels are review conventions. Codebook compliance is not proof of emotional intelligence; human raters are needed for stronger claims.</div>
    </div>
  );
}
