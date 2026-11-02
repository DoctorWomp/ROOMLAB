import { FEATURES } from "../lib/features";
import type { Boot } from "../lib/schemas";
import { Modal } from "./ui";

const cls = (s: string) => (s === "TESTED" ? "green" : s === "IMPLEMENTED" ? "cyan" : s === "PARTIAL" ? "amber" : s === "BLOCKED" ? "red" : "outline");

export default function About({ boot, onClose }: { boot: Boot; onClose: () => void }) {
  return (
    <Modal title={`About RoomLab ${boot.app_version}`} onClose={onClose} wide>
      <div className="col">
        <div className="small muted">
          Local-first multi-agent chat & simulation workbench (beta). Contract schema {boot.schema_version}. Data: <span className="mono">{boot.data_dir}</span>.
          SOMA labels, reaction lanes and confidence tags are expressive conventions — not emotion detection or verified internal states.
          Demo agents are scripted and always labelled DEMO.
        </div>
        <h3>Feature status (truthful labels)</h3>
        <table className="t">
          <thead><tr><th>Area</th><th>Feature</th><th>State</th><th>Evidence / note</th></tr></thead>
          <tbody>{FEATURES.map((f) => <tr key={f.feature}><td>{f.area}</td><td>{f.feature}</td><td><span className={`badge ${cls(f.state)}`}>{f.state}</span></td><td className="muted">{f.note}</td></tr>)}</tbody>
        </table>
        <h3>Keyboard</h3>
        <div className="small muted">Enter send · Shift+Enter new line · @ then ↑/↓ and Tab to mention · Esc closes dialogs · all controls reachable with Tab.</div>
        <h3>Licenses</h3>
        <div className="small muted">See THIRD_PARTY_NOTICES.txt. No model weights or inference binaries are included.</div>
      </div>
    </Modal>
  );
}
