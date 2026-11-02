// Safe rich-text rendering (T07). Messages are rendered as React text nodes only:
// no innerHTML, no links, no remote images. A tiny subset of Markdown (**bold**, *italic*,
// `code`) and @mentions are tokenised into elements; everything else is literal text.
import type { ReactNode } from "react";

export type Token =
  | { t: "text"; v: string }
  | { t: "bold"; v: string }
  | { t: "italic"; v: string }
  | { t: "code"; v: string }
  | { t: "mention"; v: string }
  | { t: "url"; v: string };

const PATTERN = /(\*\*[^*\n]{1,400}\*\*)|(`[^`\n]{1,400}`)|(\*[^*\n]{1,400}\*)|(@[\p{L}\p{N}_-]{1,40})|((?:https?|file|javascript|data):[^\s]{1,600})/gu;

export function tokenize(line: string): Token[] {
  const out: Token[] = [];
  let last = 0;
  for (const m of line.matchAll(PATTERN)) {
    const i = m.index ?? 0;
    if (i > last) out.push({ t: "text", v: line.slice(last, i) });
    const s = m[0];
    if (m[1]) out.push({ t: "bold", v: s.slice(2, -2) });
    else if (m[2]) out.push({ t: "code", v: s.slice(1, -1) });
    else if (m[3]) out.push({ t: "italic", v: s.slice(1, -1) });
    else if (m[4]) out.push({ t: "mention", v: s });
    else out.push({ t: "url", v: s });
    last = i + s.length;
  }
  if (last < line.length) out.push({ t: "text", v: line.slice(last) });
  return out;
}

function renderToken(tok: Token, key: number): ReactNode {
  switch (tok.t) {
    case "bold":
      return <strong key={key}>{tok.v}</strong>;
    case "italic":
      return <em key={key}>{tok.v}</em>;
    case "code":
      return <code key={key}>{tok.v}</code>;
    case "mention":
      return <span key={key} className="mention">{tok.v}</span>;
    case "url":
      // Shown as inert text: RoomLab never opens links or loads remote content from chat.
      return <span key={key} className="inert-url" title="Links in chat are not opened by RoomLab">{tok.v}</span>;
    default:
      return tok.v;
  }
}

export function RichText({ text }: { text: string }) {
  const lines = text.split("\n");
  return (
    <>
      {lines.map((l, i) => (
        <span key={i} className="rt-line" dir="auto">
          {tokenize(l).map(renderToken)}
          {i < lines.length - 1 ? <br /> : null}
        </span>
      ))}
    </>
  );
}

/** Mention suggestions for the composer (`@` prefix). */
export function mentionQuery(text: string, caret: number): string | null {
  const before = text.slice(0, caret);
  const m = before.match(/@([\p{L}\p{N}_-]{0,40})$/u);
  return m ? m[1] : null;
}
