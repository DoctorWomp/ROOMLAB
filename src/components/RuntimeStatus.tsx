import type { RuntimeStatus as RS } from "../lib/schemas";

// Distinguishes alive identity, queued job, active generation and loaded model (T27).
// "Model loaded" is shown only if the provider reported it; otherwise "unknown". No cache-rate gauge.
export default function RuntimeStatus({ status, pendingSpawns, drafts }: { status: RS | null; pendingSpawns: number; drafts: number }) {
  if (!status) return <footer className="statusbar">Loading status…</footer>;
  const s = status.status;
  const loaded = status.provider_kind === "DEMO" ? "none (Demo)" : status.model_loaded == null ? "unknown (provider does not report)" : status.model_loaded ? "yes" : "no";
  return (
    <footer className="statusbar" aria-label="Runtime status">
      <span><b>{s.run_name}</b> · {s.session_type === "CONTROLLED_TRIAL" ? "controlled trial" : "sandbox"}{s.mode !== "LIVE" ? ` · ${s.mode}` : ""}</span>
      <span>{s.paused ? <b className="warn">❚❚ paused</b> : <span className="ok">▶ running</span>}</span>
      <span title="Persistent identities (no compute while idle)"><b>{s.identities_active}</b> agents · {s.identities_dormant} dormant · {s.children_active} sub-agents</span>
      <span title="Turns waiting for the single generation slot"><b>{s.queued_jobs}</b> queued</span>
      <span title="Turns being processed right now (concurrency 1)"><b>{s.running_jobs}</b> active{drafts ? " · streaming" : ""}</span>
      <span title="Is the model resident in the provider?">model loaded: {loaded}</span>
      <span title="Run budget (turns / output tokens)">budget {s.used_turns}/{s.budget_turns} turns · {s.used_output_tokens}/{s.budget_output_tokens} tok</span>
      {pendingSpawns > 0 && <span className="warn">{pendingSpawns} spawn request(s) awaiting approval</span>}
      <span className={s.errors ? "err" : ""}>{s.errors} errors</span>
      <span>event #{s.last_seq}</span>
    </footer>
  );
}
