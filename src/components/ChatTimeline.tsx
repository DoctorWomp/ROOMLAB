import { useEffect, useMemo, useRef, useState } from "react";
import { api, errText } from "../lib/api";
import type { DraftUpdate, MessageView, RoomInfo } from "../lib/schemas";
import { RichText } from "../lib/text";
import { KindTag } from "./ui";

function Attachment({ runId, id }: { runId: string; id: string }) {
  const [src, setSrc] = useState<string | null>(null);
  const [err, setErr] = useState<string | null>(null);
  useEffect(() => {
    api.getAttachment(runId, id).then(setSrc).catch((e) => setErr(errText(e)));
  }, [runId, id]);
  if (err) return <div className="small faint">[image unavailable to this view]</div>;
  return src ? <img className="att" src={src} alt="Attached image (shown to you; text-only agents cannot see it)" /> : <div className="small faint">loading image…</div>;
}

const confClass = (c?: string | null) => (c === "HELD_OPEN" ? "amber" : c === "COMMIT" ? "green" : "cyan");

export default function ChatTimeline({ runId, room, tick, drafts, onInspect }: { runId: string; room: RoomInfo; tick: number; drafts: DraftUpdate[]; onInspect: (id: string) => void }) {
  const [msgs, setMsgs] = useState<MessageView[]>([]);
  const [err, setErr] = useState<string | null>(null);
  const [filter, setFilter] = useState("");
  const [kind, setKind] = useState("ALL");
  const ref = useRef<HTMLDivElement>(null);
  const stick = useRef(true);

  useEffect(() => {
    let alive = true;
    api.listMessages(runId, room.id).then((m) => alive && (setMsgs(m), setErr(null))).catch((e) => alive && setErr(errText(e)));
    return () => { alive = false; };
  }, [runId, room.id, tick]);

  useEffect(() => {
    const el = ref.current;
    if (el && stick.current) el.scrollTop = el.scrollHeight;
  }, [msgs, drafts]);

  const shown = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return msgs.filter((m) => (kind === "ALL" || m.author_kind === kind) && (!f || m.text.toLowerCase().includes(f) || m.author_name.toLowerCase().includes(f)));
  }, [msgs, filter, kind]);

  return (
    <>
      <div className="chat-tools">
        <input placeholder="Search messages or speaker…" value={filter} onChange={(e) => setFilter(e.target.value)} style={{ flex: 1, minHeight: 26 }} aria-label="Filter messages" />
        <select value={kind} onChange={(e) => setKind(e.target.value)} aria-label="Filter by author type" style={{ minHeight: 26 }}>
          {["ALL", "HUMAN", "AGENT", "NARRATOR", "WORLD", "TOOL", "SYSTEM"].map((k) => <option key={k}>{k}</option>)}
        </select>
        <span className="small faint">{shown.length}/{msgs.length}</span>
      </div>
      <div className="chat" ref={ref} onScroll={(e) => { const el = e.currentTarget; stick.current = el.scrollHeight - el.scrollTop - el.clientHeight < 60; }} role="log" aria-live="polite">
        {err && <div className="err small">{err}</div>}
        {!shown.length && !err && <div className="empty">No messages yet. Say hello — agents respond on their own schedule; silence is allowed.</div>}
        {shown.map((m) => {
          const k = m.author_kind;
          const cls = k === "WORLD" ? "world" : k === "NARRATOR" ? "narrator" : k === "SYSTEM" ? "system" : k === "TOOL" ? "tool" : "";
          const icon = m.author_icon || (k === "WORLD" ? "🌍" : k === "TOOL" ? "🔧" : k === "SYSTEM" ? "⚙" : "📜");
          return (
            <article key={m.message_id} className={`msg ${cls} ${m.private ? "private" : ""}`} aria-label={`${k} message from ${m.author_name}`}>
              <button className="avatar" style={{ border: 0, cursor: k === "AGENT" || k === "HUMAN" ? "pointer" : "default" }} onClick={() => (k === "AGENT" || k === "HUMAN") && onInspect(m.author_id)} aria-label={`Inspect ${m.author_name}`}>{icon}</button>
              <div>
                <div className="head">
                  <span className="name">{m.author_name}</span>
                  <KindTag kind={k} />
                  {m.is_demo && <span className="badge amber" title="Produced by a deterministic scripted policy, not a model">DEMO</span>}
                  {m.model_id && <span className="badge cyan" title="Model that produced this message">{m.model_id}</span>}
                  {m.confidence && <span className={`badge ${confClass(m.confidence)}`} title="Communication label, not a calibrated probability">{m.confidence}</span>}
                  {m.private && <span className="badge outline" title={`Visibility: ${m.visibility}`}>🔒 {m.visibility === "OPERATOR_ONLY" ? "operator only" : `→ ${m.recipient_names.join(", ") || "limited audience"}`}</span>}
                  <span className="faint small">#{m.seq}</span>
                </div>
                <div className="body"><RichText text={m.text} /></div>
                {m.reaction_lane && (
                  <div className="lane" title="Reaction lane: an outward expressive trajectory, not a verified feeling">
                    <span className="owner">{m.reaction_lane.owner === "CHARACTER" ? `Character: ${m.reaction_lane.owner_name}` : m.reaction_lane.owner === "READER_HYPOTHETICAL" ? "Reader (hypothetical)" : "SOMA (expression)"}</span>
                    {m.reaction_lane.steps.join(" → ")}
                  </div>
                )}
                {m.attachments.map((a) => <Attachment key={a} runId={runId} id={a} />)}
              </div>
            </article>
          );
        })}
        {drafts.map((d) => (
          <article key={d.job_id} className="msg draft" aria-label={`${d.agent_name} is drafting`}>
            <div className="avatar">✍</div>
            <div>
              <div className="head"><span className="name">{d.agent_name}</span><span className="badge cyan pulse">streaming · unvalidated draft</span></div>
              <div className="body mono small">{d.text || "…"}</div>
            </div>
          </article>
        ))}
      </div>
    </>
  );
}
