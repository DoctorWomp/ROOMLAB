import { useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { api, errText } from "../lib/api";
import type { Boot, ProviderConfig, ProviderHealth, RoomInfo, RoomSettings } from "../lib/schemas";
import { Modal, useNotify, useRun } from "./ui";

// Simple vs Advanced is progressive disclosure only; it never changes privileges (§3.2).
export default function SimpleAdvancedSettings({ boot, runId, room, onClose, onSaved }: { boot: Boot; runId: string; room: RoomInfo | null; onClose: () => void; onSaved: () => void }) {
  const notify = useNotify();
  const run = useRun();
  const s = boot.settings;
  const [mode, setMode] = useState(s.settings_mode);
  const [spawnMode, setSpawnMode] = useState(s.spawn_mode);
  const [maxChildren, setMaxChildren] = useState(s.max_children);
  const [soma, setSoma] = useState(s.soma_default);
  const [delay, setDelay] = useState(s.demo_turn_delay_ms);
  const [motion, setMotion] = useState(s.reduced_motion);
  const [pauseCancels, setPauseCancels] = useState(s.pause_cancels_inflight);
  const [prov, setProv] = useState<ProviderConfig>(s.provider);
  const [key, setKey] = useState("");
  const [health, setHealth] = useState<ProviderHealth | null>(null);
  const [rs, setRs] = useState<RoomSettings | null>(room?.settings ?? null);
  const [exe, setExe] = useState(s.managed_llama_exe ?? "");
  const [gguf, setGguf] = useState(s.managed_model_path ?? "");
  const [ctx, setCtx] = useState(4096);
  const [managed, setManaged] = useState<Record<string, unknown> | null>(null);
  const [busy, setBusy] = useState(false);
  const adv = mode === "ADVANCED";
  const remote = !/^https?:\/\/(127\.0\.0\.1|localhost|\[::1\])(:|\/|$)/i.test(prov.base_url);

  useEffect(() => { api.managedStatus().then(setManaged).catch(() => {}); }, []);

  const saveGeneral = () => run(async () => {
    await api.saveSettings({ settings_mode: mode, spawn_mode: spawnMode, max_children: maxChildren, soma_default: soma, demo_turn_delay_ms: delay, reduced_motion: motion, pause_cancels_inflight: pauseCancels });
    onSaved();
  }, "Settings saved");

  const test = async () => {
    setBusy(true);
    try {
      const h = await api.providerTest(prov.base_url, key || null, prov.remote_acknowledged);
      setHealth(h);
      if (h.ok && !prov.model_id && h.models[0]) setProv({ ...prov, model_id: h.models[0].id });
    } catch (e) {
      notify(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <Modal title="Settings" onClose={onClose} wide>
      <div className="col">
        <div className="row">
          <div className="tabs">
            <button className={`tab ${!adv ? "sel" : ""}`} onClick={() => setMode("SIMPLE")}>Simple</button>
            <button className={`tab ${adv ? "sel" : ""}`} onClick={() => setMode("ADVANCED")}>Advanced</button>
          </div>
          <span className="small faint">Switching views never changes permissions or research conditions.</span>
        </div>

        <h3>Inference</h3>
        <div className="card col">
          <div className="grid3">
            <label className="field">Provider
              <select value={prov.provider_kind} onChange={(e) => setProv({ ...prov, provider_kind: e.target.value as ProviderConfig["provider_kind"] })}>
                <option value="DEMO">Demo only (no model)</option>
                <option value="LOCAL_CONNECTED">Connected local (LM Studio / Ollama / OpenAI-compatible)</option>
                <option value="MANAGED_LOCAL">Managed llama-server (started by RoomLab)</option>
                {adv && <option value="CLOUD_OPT_IN">Remote / cloud (opt-in)</option>}
              </select>
            </label>
            <label className="field">Endpoint URL<input value={prov.base_url} onChange={(e) => setProv({ ...prov, base_url: e.target.value })} disabled={prov.provider_kind === "DEMO" || prov.provider_kind === "MANAGED_LOCAL"} /></label>
            <label className="field">API key (optional; memory only, never saved)<input type="password" value={key} onChange={(e) => setKey(e.target.value)} placeholder={boot.secret_set ? "•••• set for this session" : "none"} /></label>
          </div>
          {remote && prov.provider_kind !== "DEMO" && prov.provider_kind !== "MANAGED_LOCAL" && (
            <label className="check warn"><input type="checkbox" checked={prov.remote_acknowledged} onChange={(e) => setProv({ ...prov, remote_acknowledged: e.target.checked })} />
              This is not a local address. I understand compiled prompts for model-backed agents will leave this computer. (Costs are shown as raw tokens; price unknown.)</label>
          )}
          <div className="row">
            <button className="btn cyan" onClick={test} disabled={busy || prov.provider_kind === "DEMO" || prov.provider_kind === "MANAGED_LOCAL"}>Test /models</button>
            {health && <span className={health.ok ? "ok small" : "err small"}>{health.ok ? "✔" : "✖"} {health.diagnosis}</span>}
          </div>
          <div className="grid3">
            <label className="field">Model
              {health?.ok ? (
                <select value={prov.model_id ?? ""} onChange={(e) => setProv({ ...prov, model_id: e.target.value })}>{health.models.map((m) => <option key={m.id} value={m.id}>{m.id}{m.loaded ? " (loaded)" : ""}</option>)}</select>
              ) : <input value={prov.model_id ?? ""} onChange={(e) => setProv({ ...prov, model_id: e.target.value })} placeholder="Test the endpoint to list models" />}
            </label>
            <label className="field">Reasoning allowance (tokens) — thinking models need 800–1500
              <input type="number" min={0} max={8000} value={prov.reasoning_allowance_tokens} onChange={(e) => setProv({ ...prov, reasoning_allowance_tokens: Number(e.target.value) })} />
            </label>
            {adv && <label className="field">Temperature<input type="number" step={0.1} min={0} max={2} value={prov.temperature} onChange={(e) => setProv({ ...prov, temperature: Number(e.target.value) })} /></label>}
            {adv && <label className="field">Timeout (s)<input type="number" min={5} max={900} value={prov.timeout_secs} onChange={(e) => setProv({ ...prov, timeout_secs: Number(e.target.value) })} /></label>}
          </div>
          <div className="row" style={{ justifyContent: "flex-end" }}>
            <button className="btn primary" onClick={() => run(async () => { await api.providerSet({ ...prov, api_key: key || null }); onSaved(); }, "Inference settings saved")}>Save inference</button>
          </div>
          <div className="small faint">Agents use the model only if their cognition is L1/L2 with route “local/default” (edit an agent to switch). If the provider is unavailable, RoomLab reports it and never substitutes another model or invents replies.</div>
        </div>

        {adv && (
          <>
            <h3>Managed llama-server (optional)</h3>
            <div className="card col">
              <div className="small muted">Select your own llama-server.exe (and its DLLs in the same folder) and a .gguf model. RoomLab binds it to 127.0.0.1 on a free port with a one-time key, and stops only that process on exit. No binary or model is bundled.</div>
              <div className="row">
                <input className="grow" value={exe} onChange={(e) => setExe(e.target.value)} placeholder="…\llama-server.exe" />
                <button className="btn sm" onClick={async () => { const p = await open({ filters: [{ name: "llama-server", extensions: ["exe"] }] }); if (typeof p === "string") setExe(p); }}>Browse…</button>
              </div>
              <div className="row">
                <input className="grow" value={gguf} onChange={(e) => setGguf(e.target.value)} placeholder="…\model.gguf" />
                <button className="btn sm" onClick={async () => { const p = await open({ filters: [{ name: "GGUF model", extensions: ["gguf"] }] }); if (typeof p === "string") setGguf(p); }}>Browse…</button>
                <label className="field">ctx<input type="number" value={ctx} onChange={(e) => setCtx(Number(e.target.value))} style={{ width: 90 }} /></label>
              </div>
              <div className="row">
                <button className="btn primary sm" disabled={busy || !exe || !gguf} onClick={async () => { setBusy(true); const r = await run(() => api.managedStart(exe, gguf, ctx), "llama-server ready"); setBusy(false); if (r) { setManaged(r); onSaved(); } }}>{busy ? "Starting…" : "Start"}</button>
                <button className="btn sm danger" onClick={() => run(async () => { await api.managedStop(); setManaged(null); }, "Stopped (only the RoomLab-owned process)")}>Stop</button>
              </div>
              {managed && <pre className="json">{JSON.stringify(managed, null, 2)}</pre>}
            </div>
          </>
        )}

        <h3>Room defaults</h3>
        <div className="card grid3">
          <label className="field">Sub-agent spawning<select value={spawnMode} onChange={(e) => setSpawnMode(e.target.value as typeof spawnMode)}><option value="OFF">OFF</option><option value="ASK">ASK (approval required)</option><option value="AUTO_WITH_CAP">AUTO with cap</option></select></label>
          <label className="field">Max active children per room<input type="number" min={0} max={10} value={maxChildren} onChange={(e) => setMaxChildren(Number(e.target.value))} /></label>
          <label className="field">SOMA expression default<select value={soma} onChange={(e) => setSoma(e.target.value as typeof soma)}>{["OFF", "LIGHT", "STANDARD", "EXPRESSIVE"].map((m) => <option key={m}>{m}</option>)}</select></label>
          <label className="field">Demo pacing (ms between scripted turns)<input type="number" min={0} max={5000} value={delay} onChange={(e) => setDelay(Number(e.target.value))} /></label>
          <label className="field">Motion<select value={motion} onChange={(e) => setMotion(e.target.value)}><option value="FOLLOW_OS">Follow Windows setting</option><option value="REDUCE">Always reduce</option></select></label>
          {adv && <label className="check"><input type="checkbox" checked={pauseCancels} onChange={(e) => setPauseCancels(e.target.checked)} /> Pause also cancels the in-flight generation</label>}
        </div>
        <div className="row" style={{ justifyContent: "flex-end" }}><button className="btn primary" onClick={saveGeneral}>Save</button></div>

        {adv && room && rs && (
          <>
            <h3>Current room: {room.name} (changes are journaled interventions)</h3>
            <div className="card grid3">
              <label className="field">Spawn policy<select value={rs.spawn_policy} onChange={(e) => setRs({ ...rs, spawn_policy: e.target.value as RoomSettings["spawn_policy"] })}><option>DISABLED</option><option>APPROVAL_REQUIRED</option><option>UNIVERSAL_CAPPED</option></select></label>
              <label className="field">Max active children<input type="number" min={0} max={10} value={rs.max_active_children} onChange={(e) => setRs({ ...rs, max_active_children: Number(e.target.value) })} /></label>
              <label className="field">Max per parent<input type="number" min={0} max={5} value={rs.max_children_per_parent} onChange={(e) => setRs({ ...rs, max_children_per_parent: Number(e.target.value) })} /></label>
              <label className="field">Child max turns<input type="number" min={1} max={50} value={rs.child_max_turns} onChange={(e) => setRs({ ...rs, child_max_turns: Number(e.target.value) })} /></label>
              <label className="field">Child TTL (s)<input type="number" min={10} max={3600} value={rs.child_ttl_seconds} onChange={(e) => setRs({ ...rs, child_ttl_seconds: Number(e.target.value) })} /></label>
              <label className="field">Narrative resolution<select value={rs.narrative_resolution} onChange={(e) => setRs({ ...rs, narrative_resolution: e.target.value as RoomSettings["narrative_resolution"] })}><option>MINIMAL</option><option>BALANCED</option><option>CINEMATIC</option></select></label>
              <label className="check"><input type="checkbox" checked={rs.soma_enabled} onChange={(e) => setRs({ ...rs, soma_enabled: e.target.checked })} /> SOMA expression enabled</label>
              <label className="check"><input type="checkbox" checked={rs.plain_text_as_speak} onChange={(e) => setRs({ ...rs, plain_text_as_speak: e.target.checked })} /> Accept plain prose as SPEAK</label>
              <label className="check"><input type="checkbox" checked={rs.read_only} onChange={(e) => setRs({ ...rs, read_only: e.target.checked })} /> Read-only for agents</label>
            </div>
            <div className="row" style={{ justifyContent: "flex-end" }}><button className="btn" onClick={() => run(() => api.updateRoomSettings(runId, room.id, rs), "Room settings saved (intervention journaled)")}>Save room settings</button></div>
          </>
        )}
      </div>
    </Modal>
  );
}
