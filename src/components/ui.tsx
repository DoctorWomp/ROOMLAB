import { createContext, useContext, useEffect, useRef, type ReactNode } from "react";
import { errText } from "../lib/api";

export function Modal({ title, onClose, children, wide }: { title: ReactNode; onClose: () => void; children: ReactNode; wide?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const k = (e: KeyboardEvent) => e.key === "Escape" && onClose();
    window.addEventListener("keydown", k);
    ref.current?.focus();
    return () => window.removeEventListener("keydown", k);
  }, [onClose]);
  return (
    <div className="overlay" onMouseDown={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal" role="dialog" aria-modal="true" aria-label={typeof title === "string" ? title : undefined} ref={ref} tabIndex={-1} style={wide ? { width: "min(1100px, 100%)" } : undefined}>
        <div className="mh">
          <h2 style={{ margin: 0 }}>{title}</h2>
          <button className="btn ghost" onClick={onClose} aria-label="Close dialog">✕</button>
        </div>
        <div className="mb">{children}</div>
      </div>
    </div>
  );
}

export type Notify = (msg: string, err?: boolean) => void;
export const NotifyCtx = createContext<Notify>(() => {});
export const useNotify = () => useContext(NotifyCtx);

/** Run an async action and report errors as a toast. Returns the result or undefined. */
export function useRun() {
  const notify = useNotify();
  return async <T,>(f: () => Promise<T>, ok?: string): Promise<T | undefined> => {
    try {
      const r = await f();
      if (ok) notify(ok);
      return r;
    } catch (e) {
      notify(errText(e), true);
      return undefined;
    }
  };
}

export function KindTag({ kind }: { kind: string }) {
  return <span className={`kind ${kind}`}>{kind}</span>;
}

export function StatusDot({ status }: { status: string }) {
  const c = status === "ACTIVE" ? "on" : status === "DORMANT" ? "dormant" : "off";
  return <span className={`dot ${c}`} role="img" aria-label={status.toLowerCase()} title={status} />;
}

export function CognitionBadge({ c, route }: { c: string; route: string }) {
  if (c === "L0_SCRIPTED") return <span className="badge amber" title="Deterministic scripted policy — no model">DEMO</span>;
  return <span className="badge cyan" title={`Model-backed (${c}) via ${route}`}>{c.split("_")[0]} · model</span>;
}

export const roomIcon = (k: string) => (k === "COMMONS" ? "💬" : k === "GUILD" ? "🧩" : k === "STORY" ? "📜" : "🧪");
