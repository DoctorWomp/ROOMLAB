import { useMemo, useRef, useState } from "react";
import { api } from "../lib/api";
import type { RoomInfo } from "../lib/schemas";
import { mentionQuery } from "../lib/text";
import { useRun } from "./ui";

const toB64 = (buf: ArrayBuffer) => {
  let s = "";
  const b = new Uint8Array(buf);
  for (let i = 0; i < b.length; i += 0x8000) s += String.fromCharCode(...b.subarray(i, i + 0x8000));
  return btoa(s);
};

export default function MessageComposer({ runId, room, disabledReason }: { runId: string; room: RoomInfo; disabledReason: string | null }) {
  const run = useRun();
  const [text, setText] = useState("");
  const [to, setTo] = useState<string>("");
  const [atts, setAtts] = useState<{ id: string; name: string }[]>([]);
  const [caret, setCaret] = useState(0);
  const [sel, setSel] = useState(0);
  const [sending, setSending] = useState(false);
  const fileRef = useRef<HTMLInputElement>(null);
  const taRef = useRef<HTMLTextAreaElement>(null);
  const others = room.members.filter((m) => m.kind !== "HUMAN" && m.status !== "ARCHIVED");
  const q = mentionQuery(text, caret);
  const suggestions = useMemo(() => (q === null ? [] : others.filter((m) => m.display_name.toLowerCase().startsWith(q.toLowerCase())).slice(0, 6)), [q, others]);

  const reason = disabledReason ?? (sending ? "Sending…" : !text.trim() && !atts.length ? "Type a message" : null);

  const send = async () => {
    if (reason) return;
    setSending(true);
    const ok = await run(() => api.postMessage(runId, room.id, text, to ? "DIRECT" : "ROOM_MEMBERS", to ? [to] : [], atts.map((a) => a.id)));
    setSending(false);
    if (ok) {
      setText("");
      setAtts([]);
    }
  };

  const pick = (name: string) => {
    const before = text.slice(0, caret).replace(/@[\p{L}\p{N}_-]*$/u, `@${name} `);
    const next = before + text.slice(caret);
    setText(next);
    setCaret(before.length);
    setTimeout(() => taRef.current?.setSelectionRange(before.length, before.length), 0);
  };

  const onFile = async (f: File | undefined) => {
    if (!f) return;
    if (f.size > 8 * 1024 * 1024) return run(() => Promise.reject("Image too large (max 8 MB)"));
    const data = toB64(await f.arrayBuffer());
    const r = await run(() => api.attachImage(runId, room.id, f.name, data));
    if (r) setAtts((a) => [...a, { id: r.id, name: `${f.name} (${r.width}×${r.height})` }]);
  };

  return (
    <div className="composer">
      {suggestions.length > 0 && (
        <div className="mention-pop" role="listbox" aria-label="Mention suggestions">
          {suggestions.map((m, i) => (
            <button key={m.agent_id} role="option" aria-selected={i === sel} className={i === sel ? "sel" : ""} onMouseDown={(e) => { e.preventDefault(); pick(m.display_name); }}>
              <span>{m.icon}</span><span>{m.display_name}</span><span className="faint small">{m.role}</span>
            </button>
          ))}
        </div>
      )}
      {atts.length > 0 && (
        <div className="row small" style={{ marginBottom: 6 }}>
          {atts.map((a) => <span key={a.id} className="badge outline">🖼 {a.name} <button className="btn ghost sm" onClick={() => setAtts(atts.filter((x) => x.id !== a.id))} aria-label="Remove attachment">✕</button></span>)}
          <span className="faint">Text-only agents receive only “image_attachment_present”.</span>
        </div>
      )}
      <div className="row" style={{ alignItems: "flex-end" }}>
        <button className="btn" onClick={() => fileRef.current?.click()} disabled={!!disabledReason} aria-label="Attach image" title="Attach image (PNG, JPEG, WebP, GIF ≤ 8 MB)">🖼</button>
        <input ref={fileRef} type="file" accept="image/png,image/jpeg,image/webp,image/gif" hidden onChange={(e) => { onFile(e.target.files?.[0]); e.target.value = ""; }} />
        <textarea
          ref={taRef}
          className="grow"
          rows={1}
          value={text}
          placeholder={disabledReason ?? (room.kind === "STORY" ? `Say something in-world (only characters in your location hear you)…` : `Message #${room.name} — @mention to address someone…`)}
          disabled={!!disabledReason}
          onChange={(e) => { setText(e.target.value); setCaret(e.target.selectionStart); setSel(0); }}
          onKeyUp={(e) => setCaret(e.currentTarget.selectionStart)}
          onClick={(e) => setCaret(e.currentTarget.selectionStart)}
          onKeyDown={(e) => {
            if (suggestions.length) {
              if (e.key === "ArrowDown") { e.preventDefault(); setSel((sel + 1) % suggestions.length); return; }
              if (e.key === "ArrowUp") { e.preventDefault(); setSel((sel - 1 + suggestions.length) % suggestions.length); return; }
              if (e.key === "Tab" || (e.key === "Enter" && !e.shiftKey)) { e.preventDefault(); pick(suggestions[sel].display_name); return; }
            }
            if (e.key === "Enter" && !e.shiftKey) { e.preventDefault(); send(); }
          }}
          aria-label="Message"
        />
        <select value={to} onChange={(e) => setTo(e.target.value)} aria-label="Visibility" title="Visibility" disabled={!!disabledReason}>
          <option value="">{room.kind === "STORY" ? "📍 Nearby" : "👥 Everyone"}</option>
          {others.map((m) => <option key={m.agent_id} value={m.agent_id}>🔒 Direct → {m.display_name}</option>)}
        </select>
        <button className="btn primary" onClick={send} disabled={!!reason} title={reason ?? "Send (Enter). Shift+Enter for a new line."}>Send ➤</button>
      </div>
      {disabledReason && <div className="why">{disabledReason}</div>}
    </div>
  );
}
