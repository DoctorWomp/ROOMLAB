import { useEffect, useState } from "react";
import { api, errText } from "../lib/api";
import type { ActionKind, RoomInfo, ViewRole } from "../lib/schemas";
import { useRun } from "./ui";

// Fixed layout for the bundled Forgotten Keep; other worlds fall back to a grid.
const KEEP_POS: Record<string, [number, number]> = { courtyard: [70, 60], great_hall: [230, 60], library: [390, 60], cellar: [230, 160], crypt: [230, 255] };

export default function StoryPanel({ runId, room, tick, viewRole }: { runId: string; room: RoomInfo; tick: number; viewRole: ViewRole }) {
  const run = useRun();
  const [s, setS] = useState<Record<string, any> | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [target, setTarget] = useState("room");
  const [item, setItem] = useState("");
  const [last, setLast] = useState<string | null>(null);

  useEffect(() => { api.storyView(runId, room.id).then((x) => { setS(x); setErr(null); }).catch((e) => setErr(errText(e))); }, [runId, room.id, tick]);
  if (err) return <div className="err small">{err}</div>;
  if (!s) return <div className="faint">Loading world…</div>;

  const view = s.view as null | { location_id: string; name: string; ambient: string; exits: { to: string; name: string; locked: boolean; secret: boolean }[]; objects: { id: string; name: string; state: string }[]; characters: string[]; inventory: { id: string; name: string }[] };
  const locs = s.locations as [string, string][];
  const truth = s.truth as undefined | { entities: any[]; edges: any[]; inventories: [string, string][] };
  const pos = (id: string, i: number): [number, number] => KEEP_POS[id] ?? [70 + (i % 3) * 160, 60 + Math.floor(i / 3) * 95];
  const act = async (a: ActionKind, args: Record<string, unknown>) => {
    const r = await run(() => api.humanAction(runId, room.id, a, args));
    if (r) setLast(r.accepted ? `✔ ${a} accepted — ${r.detail ?? ""}` : `✖ ${a} rejected — ${r.reason_code}: ${r.detail ?? ""}`);
  };
  const edges: { from: string; to: string; hidden: boolean }[] = truth ? truth.edges.map((e: any) => ({ from: e.from, to: e.to, hidden: e.hidden })) : [];
  const known = new Set(view?.exits.map((e) => e.to) ?? []);

  return (
    <div className="col">
      <div className="row">
        <h2 style={{ margin: 0 }}>{s.world.title}</h2>
        <span className="badge">{s.world.genre}</span>
        <span className="badge violet">narration {s.world.resolution}</span>
        <span style={{ flex: 1 }} />
        <button className="btn primary sm" onClick={() => run(() => api.storyTick(runId, room.id), "World advanced one tick: each character takes a turn")}>⏭ Advance world tick</button>
      </div>
      {viewRole === "OPERATOR" && <div className="banner truth" style={{ borderRadius: 6 }}>OPERATOR TRUTH — full map, hidden passages and every entity. Characters never receive this.</div>}
      <svg className="map" viewBox="0 0 470 300" role="img" aria-label="World map">
        {(viewRole === "OPERATOR" ? edges : (view?.exits ?? []).map((e) => ({ from: view!.location_id, to: e.to, hidden: e.secret }))).map((e, i) => {
          const a = pos(e.from, locs.findIndex((l) => l[0] === e.from)), b = pos(e.to, locs.findIndex((l) => l[0] === e.to));
          return <line key={i} x1={a[0]} y1={a[1]} x2={b[0]} y2={b[1]} className={e.hidden ? "secret" : ""} />;
        })}
        {locs.map(([id, name], i) => {
          const [x, y] = pos(id, i);
          const visibleToMe = viewRole === "OPERATOR" || id === view?.location_id || known.has(id) || !edges.length;
          if (!visibleToMe && id === "crypt") return null;
          const ents = truth?.entities.filter((e: any) => e.location_id === id && e.kind !== "SOUND") ?? [];
          return (
            <g key={id} className={`loc ${id === view?.location_id ? "here" : ""}`}>
              <rect x={x - 62} y={y - 22} width="124" height="44" rx="9" />
              <text x={x} y={y - 3} textAnchor="middle">{name}</text>
              {viewRole === "OPERATOR" && <text x={x} y={y + 13} textAnchor="middle" className="ent">{ents.map((e: any) => (e.kind === "CHARACTER" || e.kind === "FAMILIAR" ? "●" : "▫") + e.name).join(" ").slice(0, 28)}</text>}
              {id === view?.location_id && viewRole !== "OPERATOR" && <text x={x} y={y + 13} textAnchor="middle" className="ent">you are here</text>}
            </g>
          );
        })}
      </svg>
      {view ? (
        <div className="card small col">
          <div><b>{view.name}</b> — <span className="muted">{view.ambient}</span></div>
          <div>You see: {view.objects.length ? view.objects.map((o) => o.name + (o.state ? ` (${o.state})` : "")).join(", ") : "nothing notable"}</div>
          <div>Present: {view.characters.length ? view.characters.join(", ") : "no one"}</div>
          <div>You carry: {view.inventory.length ? view.inventory.map((o) => o.name).join(", ") : "nothing"}</div>
          <div className="row">
            <b>Move:</b>
            {view.exits.map((e) => <button key={e.to} className="btn sm" disabled={e.locked} onClick={() => act("MOVE", { to: e.to })} title={e.locked ? "Locked" : `Go to ${e.name}`}>{e.secret ? "🕳 " : "→ "}{e.name}{e.locked ? " 🔒" : ""}</button>)}
          </div>
          <div className="row">
            <button className="btn sm" onClick={() => act("LOOK", {})}>👁 Look</button>
            <select value={target} onChange={(e) => setTarget(e.target.value)} aria-label="Target">
              <option value="room">the room (search)</option>
              {view.objects.map((o) => <option key={o.id} value={o.id}>{o.name}</option>)}
              {view.inventory.map((o) => <option key={o.id} value={o.id}>{o.name} (carried)</option>)}
            </select>
            <button className="btn sm" onClick={() => act("INSPECT", { target })}>🔍 Inspect</button>
            <button className="btn sm" disabled={target === "room"} onClick={() => act("TAKE", { target })}>✋ Take</button>
            <button className="btn sm" disabled={!view.inventory.some((o) => o.id === target)} onClick={() => act("DROP", { target })}>⤓ Drop</button>
          </div>
          <div className="row">
            <select value={item} onChange={(e) => setItem(e.target.value)} aria-label="Item to use"><option value="">use item…</option>{view.inventory.map((o) => <option key={o.id} value={o.id}>{o.name}</option>)}</select>
            <span className="faint">on</span>
            <span className="faint">{target}</span>
            <button className="btn sm" disabled={!item || target === "room"} onClick={() => act("USE", { item, target })}>⚙ Use</button>
            <button className="btn sm ghost" onClick={() => act("WAIT", {})}>⏸ Wait</button>
          </div>
          {last && <div className={last.startsWith("✔") ? "ok" : "warn"}>{last}</div>}
        </div>
      ) : (
        <div className="faint small">You are not a character in this world.</div>
      )}
      {truth && (
        <details>
          <summary className="small">Operator truth: entities & inventories</summary>
          <table className="t"><thead><tr><th>id</th><th>kind</th><th>where</th><th>state</th><th>visibility</th></tr></thead>
            <tbody>{truth.entities.map((e: any) => <tr key={e.id}><td>{e.name}</td><td>{e.kind}</td><td>{e.location_id ?? (truth.inventories.find((i) => i[1] === e.id)?.[0] ? `held by ${truth.inventories.find((i) => i[1] === e.id)![0]}` : "—")}</td><td>{e.state}</td><td>{e.visibility}</td></tr>)}</tbody></table>
        </details>
      )}
    </div>
  );
}
