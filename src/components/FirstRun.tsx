import { useState } from "react";
import { api, errText } from "../lib/api";
import type { Boot, ProviderHealth } from "../lib/schemas";
import { useNotify } from "./ui";

type Choice = "DEMO" | "LOCAL" | "EMBEDDED";

export default function FirstRun({ boot, onDone }: { boot: Boot; onDone: () => void }) {
  const notify = useNotify();
  const [choice, setChoice] = useState<Choice>("DEMO");
  const [url, setUrl] = useState(boot.settings.provider.base_url || "http://127.0.0.1:1234/v1");
  const [health, setHealth] = useState<ProviderHealth | null>(null);
  const [model, setModel] = useState("");
  const [allowance, setAllowance] = useState(0);
  const [busy, setBusy] = useState(false);

  const test = async () => {
    setBusy(true);
    try {
      const h = await api.providerTest(url, null, false);
      setHealth(h);
      if (h.models[0]) setModel(h.models.find((m) => m.loaded)?.id ?? h.models[0].id);
    } catch (e) {
      notify(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  const finish = async () => {
    setBusy(true);
    try {
      if (choice === "LOCAL") {
        if (!health?.ok || !model) throw new Error("Test the endpoint and pick a model first, or choose Explore Demo.");
        await api.providerSet({ provider_kind: "LOCAL_CONNECTED", base_url: url, model_id: model, remote_acknowledged: false, temperature: 0.7, timeout_secs: 180, reasoning_allowance_tokens: allowance });
        await api.completeFirstRun("LOCAL_CONNECTED");
      } else {
        await api.completeFirstRun("DEMO");
      }
      onDone();
    } catch (e) {
      notify(errText(e), true);
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="firstrun">
      <div className="box">
        <div className="row" style={{ gap: 12 }}>
          <span className="brand-mark" style={{ width: 36, height: 36, borderRadius: 10 }} aria-hidden />
          <div>
            <h2 style={{ fontSize: 22, margin: 0 }}>Welcome to RoomLab</h2>
            <div className="muted">A local-first chat and simulation workbench for multiple agents with separate, enforced perspectives.</div>
          </div>
        </div>
        <div className="card" style={{ marginTop: 16 }}>
          <div className="grid2 small">
            <div><b>💬 Commons</b> — flat group chat; mentions, reactions, silence allowed.</div>
            <div><b>🧩 Guild</b> — coordinator + specialists, task clusters, approved sub-agents.</div>
            <div><b>📜 Story</b> — text world with rooms, objects, hidden facts; the world engine decides outcomes.</div>
            <div><b>🧪 Crucible</b> — steelman / skeptic / mediator review, sealed intent tests.</div>
          </div>
          <div className="hr" />
          <div className="small muted">
            Each agent sees only what was delivered to it; private notes stay private. <b>Demo Mode</b> uses deterministic scripted agents (labelled DEMO) and needs no model.
            Model calls are capped by run budgets. Nothing is downloaded and no cloud call is made unless you configure it.
            Your data is saved in <span className="mono">{boot.data_dir}</span>.
          </div>
        </div>

        <div className="choice" role="radiogroup" aria-label="How do you want to start?">
          {([
            ["DEMO", "▶ Explore Demo", "Always works. Three scripted agents in a Commons room. No model needed."],
            ["LOCAL", "🔌 Connect local inference", "Use an already-running LM Studio, Ollama or other OpenAI-compatible server on this PC."],
            ["EMBEDDED", "📦 Use embedded model", "Run your own llama-server + GGUF model file. (Not bundled in this release.)"],
          ] as const).map(([k, t, d]) => (
            <button key={k} role="radio" aria-checked={choice === k} className={`card ${choice === k ? "sel" : ""}`} onClick={() => setChoice(k)}>
              <div style={{ fontWeight: 650, marginBottom: 4 }}>{t}</div>
              <div className="small muted">{d}</div>
            </button>
          ))}
        </div>

        {choice === "LOCAL" && (
          <div className="card col">
            <label className="field">Endpoint (suggested LM Studio default — change for Ollama: http://127.0.0.1:11434/v1)
              <div className="row"><input className="grow" value={url} onChange={(e) => setUrl(e.target.value)} aria-label="Endpoint URL" /><button className="btn cyan" onClick={test} disabled={busy}>Test /models</button></div>
            </label>
            {health && (
              <div className={health.ok ? "ok small" : "err small"}>{health.ok ? "✔" : "✖"} {health.diagnosis}{health.latency_ms != null ? ` (${health.latency_ms} ms)` : ""}</div>
            )}
            {health?.ok && (
              <div className="grid2">
                <label className="field">Model (reported by the server)
                  <select value={model} onChange={(e) => setModel(e.target.value)}>{health.models.map((m) => <option key={m.id} value={m.id}>{m.id}{m.loaded ? " (loaded)" : ""}</option>)}</select>
                </label>
                <label className="field">Reasoning allowance (tokens) — for thinking models like Gemma 4 / Qwen3 use 800–1500
                  <input type="number" min={0} max={8000} value={allowance} onChange={(e) => setAllowance(Number(e.target.value))} />
                </label>
              </div>
            )}
            <div className="small faint">LM Studio: Developer tab → Start Server. The app never starts or installs a server for you. Demo agents still work alongside; you can switch an agent to the real model in its profile.</div>
          </div>
        )}
        {choice === "EMBEDDED" && (
          <div className="card small muted">
            This release does not include a llama-server binary or model weights. Choose <b>Explore Demo</b> now, then open Settings → Advanced → Managed llama-server to select your own <span className="mono">llama-server.exe</span> and a <span className="mono">.gguf</span> file. RoomLab binds it to 127.0.0.1 and stops only the process it started.
          </div>
        )}
        <div className="row" style={{ marginTop: 16, justifyContent: "flex-end" }}>
          <button className="btn primary" onClick={finish} disabled={busy || choice === "EMBEDDED"} style={{ height: 36, padding: "0 18px" }}>
            {choice === "LOCAL" ? "Connect and start" : "Explore Demo"}
          </button>
          {choice === "EMBEDDED" && <button className="btn primary" onClick={() => setChoice("DEMO")}>Start in Demo first</button>}
        </div>
      </div>
    </div>
  );
}
