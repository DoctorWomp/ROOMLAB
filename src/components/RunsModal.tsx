import { useEffect, useState } from "react";
import { api } from "../lib/api";
import type { RunSummary, ScenarioDef } from "../lib/schemas";
import { Modal, roomIcon, useRun } from "./ui";

export default function RunsModal({ current, onClose, onSwitch }: { current: string; onClose: () => void; onSwitch: (id: string) => void }) {
  const run = useRun();
  const [runs, setRuns] = useState<RunSummary[]>([]);
  const [scen, setScen] = useState<ScenarioDef[]>([]);
  const [controlled, setControlled] = useState(false);
  useEffect(() => {
    api.listRuns().then(setRuns).catch(() => {});
    api.listScenarios().then(setScen).catch(() => {});
  }, []);
  return (
    <Modal title="Runs & scenario pack" onClose={onClose} wide>
      <div className="grid2">
        <div className="col">
          <h3>Runs</h3>
          <button className="btn primary sm" onClick={async () => { const id = await run(() => api.newSandboxRun("Sandbox")); if (id) onSwitch(id); }}>＋ New sandbox run</button>
          {runs.map((r) => (
            <button key={r.id} className={`nav-item ${r.id === current ? "sel" : ""}`} onClick={() => onSwitch(r.id)}>
              <span className="grow">
                <div>{r.name}</div>
                <div className="faint small">{r.session_type === "CONTROLLED_TRIAL" ? "controlled" : "sandbox"} · {r.mode} · {r.events} events · {new Date(r.created_at).toLocaleString()}</div>
              </span>
              {r.contaminated && <span className="badge red">contaminated</span>}
              {r.mode === "PLAYBACK" && <span className="badge violet">playback</span>}
            </button>
          ))}
        </div>
        <div className="col">
          <h3>Scenario pack (§15.2)</h3>
          <label className="check"><input type="checkbox" checked={controlled} onChange={(e) => setControlled(e.target.checked)} /> Start as controlled trial (interventions mark contamination)</label>
          {scen.map((s) => (
            <div key={s.id} className="card small">
              <div className="row"><b>{roomIcon(s.room)} {s.id}</b><span className="grow">{s.title}</span>
                <button className="btn sm primary" onClick={async () => { const id = await run(() => api.startScenario(s.id, controlled), `Started ${s.id}`); if (id) onSwitch(id); }}>Start</button></div>
              <div className="muted">{s.question}</div>
              <div className="faint">{s.how_to_run}</div>
              {s.requires && <div className="warn">Requires: {s.requires}</div>}
            </div>
          ))}
        </div>
      </div>
    </Modal>
  );
}
