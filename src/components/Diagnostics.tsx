import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import { Modal } from "./ui";

export default function Diagnostics({ onClose }: { onClose: () => void }) {
  const [d, setD] = useState<Record<string, any> | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => { api.diagnostics().then(setD).catch((e) => setErr(errText(e))); }, []);
  const row = (k: string, v: unknown, ok?: boolean) => (
    <tr key={k}><td className="muted">{k}</td><td className={ok === undefined ? "" : ok ? "ok" : "err"}>{v === null || v === undefined ? "—" : typeof v === "object" ? <pre className="json">{JSON.stringify(v, null, 2)}</pre> : String(v)}</td></tr>
  );
  return (
    <Modal title="Runtime & provider diagnostics" onClose={onClose} wide>
      {err && <div className="err">{err}</div>}
      {!d ? <div className="faint">Checking…</div> : (
        <div className="col">
          <table className="t"><tbody>
            {row("App version", d.app_version)}
            {row("Contract schema", d.schema_version)}
            {row("DB migration", d.db_schema_migration)}
            {row("DB integrity", d.db_integrity_ok ? "ok" : "FAILED", d.db_integrity_ok)}
            {row("Data folder", d.data_dir)}
            {row("Data writable", d.data_writable ? "yes" : "no", d.data_writable)}
            {row("Portable mode", d.portable ? "yes (data next to RoomLab.exe)" : "no")}
            {row("Notice", d.notice)}
            {row("WebView2 runtime", d.webview2_version ?? "not detected", !!d.webview2_version)}
            {row("OS", d.os)}
            {row("Executable", d.exe)}
            {row("Execution tier", d.execution_tier)}
            {row("Network listen", d.network_listen)}
            {row("Provider", d.provider)}
            {row("Provider health", d.provider_health ?? "not tested (Demo / managed / remote not acknowledged)", d.provider_health ? d.provider_health.ok : undefined)}
            {row("Managed llama-server", d.managed_llama ?? "not running")}
          </tbody></table>
          <div className="row">
            <button className="btn" onClick={() => api.openFolder("data")}>Open data folder</button>
            <button className="btn" onClick={() => api.openFolder("logs")}>Open logs</button>
            <button className="btn" onClick={() => api.openFolder("exports")}>Open exports</button>
          </div>
        </div>
      )}
    </Modal>
  );
}
