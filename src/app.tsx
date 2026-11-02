import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { api, errText, onChanged, onDraft, onRunnerError } from "./lib/api";
import type { AgentSummary, Boot, DraftUpdate, RoomInfo, RuntimeStatus as RS, SpawnRow } from "./lib/schemas";
import { NotifyCtx, roomIcon } from "./components/ui";
import FirstRun from "./components/FirstRun";
import RoomNavigator from "./components/RoomNavigator";
import ChatTimeline from "./components/ChatTimeline";
import MessageComposer from "./components/MessageComposer";
import Inspector from "./components/Inspector";
import OrgChart from "./components/OrgChart";
import StoryPanel from "./components/StoryPanel";
import CruciblePanel from "./components/CruciblePanel";
import Observatory from "./components/Observatory";
import RuntimeStatus from "./components/RuntimeStatus";
import SimpleAdvancedSettings from "./components/SimpleAdvancedSettings";
import RunsModal from "./components/RunsModal";
import Diagnostics from "./components/Diagnostics";
import About from "./components/About";
import AgentFoundry from "./components/AgentFoundry";

type View = "room" | "observatory";
type Dialog = null | "runs" | "settings" | "diagnostics" | "about" | { foundry: string | null };

export default function App() {
  const [boot, setBoot] = useState<Boot | null>(null);
  const [bootErr, setBootErr] = useState<string | null>(null);
  const [runId, setRunId] = useState<string | null>(null);
  const [rooms, setRooms] = useState<RoomInfo[]>([]);
  const [roomId, setRoomId] = useState<string | null>(null);
  const [agents, setAgents] = useState<AgentSummary[]>([]);
  const [status, setStatus] = useState<RS | null>(null);
  const [spawns, setSpawns] = useState<SpawnRow[]>([]);
  const [view, setView] = useState<View>("room");
  const [dialog, setDialog] = useState<Dialog>(null);
  const [inspect, setInspect] = useState<string | null>(null);
  const [drafts, setDrafts] = useState<Record<string, DraftUpdate>>({});
  const [tick, setTick] = useState(0);
  const [toast, setToast] = useState<{ msg: string; err: boolean } | null>(null);
  const [roomTab, setRoomTab] = useState<"chat" | "panel">("chat");
  const toastTimer = useRef<number>();

  const notify = useCallback((msg: string, err = false) => {
    setToast({ msg, err });
    window.clearTimeout(toastTimer.current);
    toastTimer.current = window.setTimeout(() => setToast(null), err ? 7000 : 3200);
  }, []);

  const loadBoot = useCallback(async () => {
    try {
      const b = await api.bootstrap();
      setBoot(b);
      setRunId(b.settings.active_run_id ?? null);
    } catch (e) {
      setBootErr(errText(e));
    }
  }, []);
  useEffect(() => { loadBoot(); }, [loadBoot]);

  // Live updates from the backend runner.
  useEffect(() => {
    let t: number | undefined;
    const un1 = onChanged(() => {
      window.clearTimeout(t);
      t = window.setTimeout(() => setTick((x) => x + 1), 90);
    });
    const un2 = onDraft((d) =>
      setDrafts((m) => {
        const n = { ...m };
        if (d.done) delete n[d.job_id];
        else n[d.job_id] = d;
        return n;
      }),
    );
    const un3 = onRunnerError((m) => notify(`Runner: ${m}`, true));
    const poll = window.setInterval(() => setTick((x) => x + 1), 4000);
    return () => {
      un1.then((f) => f());
      un2.then((f) => f());
      un3.then((f) => f());
      window.clearInterval(poll);
    };
  }, [notify]);

  const reload = useCallback(async () => {
    if (!runId) return;
    try {
      const [r, a, s, sp] = await Promise.all([api.listRooms(runId), api.listAgents(runId), api.runtimeStatus(runId), api.listSpawns(runId)]);
      setRooms(r);
      setAgents(a);
      setStatus(s);
      setSpawns(sp);
      setRoomId((cur) => (cur && r.some((x) => x.id === cur) ? cur : r[0]?.id ?? null));
    } catch (e) {
      notify(errText(e), true);
    }
  }, [runId, notify]);
  useEffect(() => { reload(); }, [reload, tick]);

  const switchRun = useCallback(async (id: string) => {
    await api.setActiveRun(id);
    setRunId(id);
    setRoomId(null);
    setInspect(null);
    setDrafts({});
    setTick((x) => x + 1);
  }, []);

  const room = useMemo(() => rooms.find((r) => r.id === roomId) ?? null, [rooms, roomId]);
  const viewRole = status?.view_role ?? boot?.view_role ?? "OPERATOR";
  const isPlayback = status?.status.mode === "PLAYBACK";
  const reduced = boot?.settings.reduced_motion === "REDUCE";

  if (bootErr) return <div className="empty"><div><h2>RoomLab could not start</h2><p className="err">{bootErr}</p></div></div>;
  if (!boot) return <div className="empty">Starting RoomLab…</div>;
  if (!boot.settings.first_run_complete || !runId) {
    return (
      <NotifyCtx.Provider value={notify}>
        <FirstRun boot={boot} onDone={loadBoot} />
        {toast && <div className={`toast ${toast.err ? "err" : ""}`} role="status">{toast.msg}</div>}
      </NotifyCtx.Provider>
    );
  }

  const control = async (a: "pause" | "resume" | "step") => {
    try { await api.runControl(runId, a); } catch (e) { notify(errText(e), true); }
  };
  const toggleRole = async () => {
    const to = viewRole === "OPERATOR" ? "PARTICIPANT" : "OPERATOR";
    try {
      await api.setViewRole(to, false);
    } catch (e) {
      const m = errText(e);
      if (m.includes("Acknowledge") && window.confirm(`${m}\n\nOpen operator truth anyway and mark this trial contaminated?`)) {
        await api.setViewRole(to, true).catch((x) => notify(errText(x), true));
      } else notify(m, true);
    }
    setTick((x) => x + 1);
  };
  const pending = spawns.filter((s) => s.state === "AWAIT_APPROVAL").length;
  const provider = boot.settings.provider;

  return (
    <NotifyCtx.Provider value={notify}>
      <div className={`shell ${inspect ? "" : "no-insp"} ${reduced ? "reduced-motion" : ""}`}>
        <header className="topbar">
          <div className="brand"><span className="brand-mark" aria-hidden /> RoomLab</div>
          <button className="btn" onClick={() => setDialog("runs")} title="Runs, scenarios, import">
            🗂 {status?.status.run_name ?? "Run"} ▾
          </button>
          {room && <span className="badge outline">{roomIcon(room.kind)} {room.name}</span>}
          <span className="sep" />
          <RuntimeBadge status={status} providerKind={provider.provider_kind} />
          <span className="sep" />
          {status?.status.paused ? (
            <button className="btn primary" onClick={() => control("resume")} disabled={isPlayback} aria-label="Run simulation">▶ Run</button>
          ) : (
            <button className="btn" onClick={() => control("pause")} disabled={isPlayback} aria-label="Pause simulation">❚❚ Pause</button>
          )}
          <button className="btn" onClick={() => control("step")} disabled={isPlayback} title="Process exactly one queued turn, then stay paused">▷ Step</button>
          <button className="btn ghost" onClick={() => api.cancelInflight().then(() => notify("Cancel requested for the in-flight generation"))} title="Cancel the generation currently streaming (nothing is committed)">⨯ Cancel gen</button>
          <button className="btn" onClick={async () => { const n = window.prompt("Name for the fork:", "Fork"); if (n !== null) { try { const f = await api.forkRun(runId, n); await switchRun(f); notify("Forked: new run id " + f); } catch (e) { notify(errText(e), true); } } }}>⤴ Fork</button>
          <span className="sep" />
          <button className={`btn ${view === "observatory" ? "active" : ""}`} onClick={() => setView(view === "observatory" ? "room" : "observatory")}>🔭 Observatory</button>
          <button className={`btn ${viewRole === "OPERATOR" ? "cyan" : ""}`} onClick={toggleRole} title="Backend-enforced view: Operator sees truth; Participant sees only what 'You' observed">
            {viewRole === "OPERATOR" ? "👁 Operator view" : "🧑 Participant view"}
          </button>
          <span style={{ flex: 1 }} />
          <button className="btn ghost" onClick={() => setDialog("settings")} aria-label="Settings">⚙ Settings</button>
          <button className="btn ghost" onClick={() => setDialog("diagnostics")}>🩺 Diagnostics</button>
          <button className="btn ghost" onClick={() => setDialog("about")}>ⓘ About</button>
        </header>

        <nav className="nav" aria-label="Rooms and members">
          <RoomNavigator runId={runId} rooms={rooms} roomId={roomId} agents={agents} spawns={spawns} onRoom={(id) => { setRoomId(id); setView("room"); setRoomTab("chat"); }} onInspect={setInspect} onNewAgent={() => setDialog({ foundry: null })} drafts={drafts} />
        </nav>

        <main className="main">
          {status?.status.contaminated && <div className="banner err">⚠ This controlled trial is CONTAMINATED (an intervention or operator-truth view occurred). See Observatory → Interventions.</div>}
          {isPlayback && <div className="banner playback">PLAYBACK run — recorded events only. No model calls, no new actions.</div>}
          {boot.notice && <div className="banner info">{boot.notice}</div>}
          {view === "observatory" ? (
            <Observatory runId={runId} rooms={rooms} agents={agents} viewRole={viewRole} tick={tick} />
          ) : room ? (
            <>
              <div className="room-head">
                <span style={{ fontSize: 18 }}>{roomIcon(room.kind)}</span>
                <span className="title">{room.name}</span>
                <span className="badge">{room.kind}</span>
                {room.kind !== "COMMONS" && (
                  <div className="tabs" role="tablist">
                    <button role="tab" aria-selected={roomTab === "chat"} className={`tab ${roomTab === "chat" ? "sel" : ""}`} onClick={() => setRoomTab("chat")}>Chat</button>
                    <button role="tab" aria-selected={roomTab === "panel"} className={`tab ${roomTab === "panel" ? "sel" : ""}`} onClick={() => setRoomTab("panel")}>
                      {room.kind === "GUILD" ? `Org & tasks${pending ? ` (${pending})` : ""}` : room.kind === "STORY" ? "World" : "Cases"}
                    </button>
                  </div>
                )}
                <span style={{ flex: 1 }} />
                {viewRole === "OPERATOR" ? <span className="badge amber" title="You see private messages and operator-only notices">OPERATOR TRUTH</span> : <span className="badge cyan">Participant: only what “You” observed</span>}
              </div>
              <div className="split">
                <div className="left">
                  <ChatTimeline runId={runId} room={room} tick={tick} drafts={Object.values(drafts).filter((d) => d.room_id === room.id)} onInspect={setInspect} />
                  <MessageComposer runId={runId} room={room} disabledReason={isPlayback ? "Playback runs are read-only" : room.settings.read_only ? "Room is read-only" : null} />
                </div>
                {room.kind !== "COMMONS" && roomTab === "panel" && (
                  <div className="right">
                    {room.kind === "GUILD" && <OrgChart runId={runId} room={room} spawns={spawns} tick={tick} />}
                    {room.kind === "STORY" && <StoryPanel runId={runId} room={room} tick={tick} viewRole={viewRole} />}
                    {room.kind === "CRUCIBLE" && <CruciblePanel runId={runId} room={room} tick={tick} viewRole={viewRole} />}
                  </div>
                )}
              </div>
            </>
          ) : (
            <div className="empty">No rooms yet — create one from the left panel.</div>
          )}
        </main>

        <aside className="insp" aria-label="Inspector">
          {inspect && <Inspector runId={runId} agentId={inspect} rooms={rooms} roomId={roomId} viewRole={viewRole} tick={tick} onClose={() => setInspect(null)} onEdit={(id) => setDialog({ foundry: id })} />}
        </aside>

        <RuntimeStatus status={status} pendingSpawns={pending} drafts={Object.keys(drafts).length} />
      </div>

      {dialog === "runs" && <RunsModal current={runId} onClose={() => setDialog(null)} onSwitch={async (id) => { await switchRun(id); setDialog(null); }} />}
      {dialog === "settings" && <SimpleAdvancedSettings boot={boot} runId={runId} room={room} onClose={() => setDialog(null)} onSaved={loadBoot} />}
      {dialog === "diagnostics" && <Diagnostics onClose={() => setDialog(null)} />}
      {dialog === "about" && <About boot={boot} onClose={() => setDialog(null)} />}
      {dialog && typeof dialog === "object" && "foundry" in dialog && (
        <AgentFoundry runId={runId} rooms={rooms} agentId={dialog.foundry} onClose={() => setDialog(null)} onSaved={() => { setDialog(null); setTick((x) => x + 1); }} />
      )}
      {toast && <div className={`toast ${toast.err ? "err" : ""}`} role="status" aria-live="polite">{toast.msg}</div>}
    </NotifyCtx.Provider>
  );
}

function RuntimeBadge({ status, providerKind }: { status: RS | null; providerKind: string }) {
  if (!status) return null;
  if (providerKind === "DEMO") return <span className="badge amber" title="No inference provider. Scripted Demo agents only.">◉ DEMO MODE</span>;
  const reach = status.provider_reachable;
  return (
    <span className={`badge ${reach === false ? "red" : "green"}`} title={`${status.endpoint} — ${status.endpoint_loopback ? "local (loopback)" : "REMOTE endpoint"}`}>
      ◉ {status.endpoint_loopback ? "LOCAL" : "REMOTE"} · {status.model_id ?? "no model selected"}
    </span>
  );
}
