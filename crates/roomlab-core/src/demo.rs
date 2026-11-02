//! Demo Mode L0 scripted policies (§0.3, §8.3). Deterministic, no model calls, and every
//! output is labelled DEMO. Policies only *propose* actions; the action gate validates them
//! exactly like model output. Policies read only what the agent itself observed.

use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Map, Value};

use crate::actions::new_request;
use crate::auth::{AgentRow, Principal};
use crate::comms::{self, MessageView};
use crate::contracts::*;
use crate::error::Result;
use crate::knowledge;
use crate::room;
use crate::scheduler::Job;
use crate::world;

pub struct DemoInput<'a> {
    pub run_id: &'a str,
    pub job: &'a Job,
    pub agent: &'a AgentRow,
    pub room_kind: RoomKind,
    pub trigger: Option<MessageView>,
    pub seq: i64,
}

struct Out {
    v: Vec<ActionRequest>,
    run: String,
    agent: String,
    seq: i64,
}

impl Out {
    fn speak(&mut self, text: &str, lane: Option<(&[&str], LaneOwner, Option<&str>)>, conf: Option<Confidence>) {
        let mut r = new_request(&self.run, &self.agent, self.seq, ActionKind::Speak);
        r.message_text = Some(text.to_string());
        r.reaction_lane = lane.map(|(steps, owner, name)| ReactionLane { owner, owner_name: name.map(String::from), steps: steps.iter().map(|s| s.to_string()).collect() });
        r.confidence = conf;
        self.v.push(r);
    }
    fn act(&mut self, kind: ActionKind, args: Value, text: Option<&str>) {
        let mut r = new_request(&self.run, &self.agent, self.seq, kind);
        if let Value::Object(m) = args {
            r.args = m;
        }
        r.message_text = text.map(String::from);
        self.v.push(r);
    }
}

#[derive(Default, Debug)]
struct Class {
    bid: bool,
    short: bool,
    question: bool,
    image: bool,
    greeting: bool,
}

fn classify(m: &MessageView) -> Class {
    let t = m.record.text.to_lowercase();
    let words = t.split_whitespace().count();
    let appreciative = ["thank", "love", "appreciate", "great", "nice", "favorite", "favourite", "helpful", "don't stop", "dont stop"].iter().any(|w| t.contains(w));
    let complaint = ["but", "ugh", "annoying", "broke", "shredded", "frustrat", "hate", "again", "why", "ruined", "destroyed"].iter().any(|w| t.contains(w));
    Class {
        bid: appreciative && complaint,
        short: words > 0 && words <= 3 && !t.contains('?'),
        question: t.trim_end().ends_with('?'),
        image: !m.record.attachments.is_empty(),
        greeting: ["hello", "hi ", "hey", "good morning"].iter().any(|w| t.starts_with(w) || t == w.trim()),
    }
}

/// Quote a short excerpt. `@` is dropped so quoting never re-mentions anyone.
fn snippet(s: &str, n: usize) -> String {
    let s = s.replace('@', "");
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n { format!("{t}…") } else { t }
}

fn author_is_agent(m: &MessageView) -> bool {
    m.record.author_kind == OriginKind::Agent
}

fn was_mentioned(conn: &Connection, run_id: &str, m: &MessageView, me: &str) -> Result<bool> {
    let ids = comms::parse_mentions(conn, run_id, &m.record.room_id, &m.record.text)?;
    Ok(ids.iter().any(|i| i == me || i == "*"))
}

fn ticks_done(conn: &Connection, run_id: &str, agent: &str, room: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND agent_id=?2 AND room_id=?3 AND status='DONE' AND reason='world tick'",
        params![run_id, agent, room],
        |r| r.get(0),
    )?)
}

fn has_obs(conn: &Connection, run_id: &str, agent: &str, kind: ObsKind, needle: &str) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM observations WHERE run_id=?1 AND observer_id=?2 AND kind=?3 AND data_json LIKE ?4 LIMIT 1",
            params![run_id, agent, kind.as_str(), format!("%{needle}%")],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn open_task(conn: &Connection, run_id: &str, room: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT cluster_id FROM tasks WHERE run_id=?1 AND room_id=?2 AND status IN ('NEW','IN_PROGRESS','BLOCKED') ORDER BY cluster_id DESC LIMIT 1",
            params![run_id, room],
            |r| r.get(0),
        )
        .optional()?)
}

fn spoke_since(conn: &Connection, run_id: &str, room: &str, author: &str, seq: i64) -> Result<bool> {
    Ok(conn
        .query_row(
            "SELECT 1 FROM messages WHERE run_id=?1 AND room_id=?2 AND author_id=?3 AND seq > ?4 AND visibility='ROOM_MEMBERS' LIMIT 1",
            params![run_id, room, author, seq],
            |_| Ok(()),
        )
        .optional()?
        .is_some())
}

fn task_created_seq(conn: &Connection, run_id: &str, cid: &str) -> Result<i64> {
    Ok(conn
        .query_row(
            "SELECT MIN(seq) FROM events WHERE run_id=?1 AND event_type='TASK_UPDATED' AND json_extract(payload_json,'$.cluster_id')=?2",
            params![run_id, cid],
            |r| r.get::<_, Option<i64>>(0),
        )?
        .unwrap_or(0))
}

/// Decide the scripted actions for a job. Empty vec = explicit NO_OP (silence).
pub fn decide(conn: &Connection, inp: &DemoInput) -> Result<Vec<ActionRequest>> {
    let mut o = Out { v: vec![], run: inp.run_id.into(), agent: inp.agent.id.clone(), seq: inp.seq };
    let policy = inp.agent.demo_policy.clone().unwrap_or_default();
    let room = inp.job.room_id.as_str();
    let me = inp.agent.id.as_str();
    let trig = inp.trigger.as_ref();
    let tick = inp.job.reason == "world tick";

    if inp.job.reason.starts_with("crucible:") || inp.job.reason.starts_with("intent:") {
        crucible_policy(conn, inp, &mut o)?;
        return Ok(o.v);
    }

    match (policy.as_str(), inp.room_kind) {
        ("echo", _) => {
            if let Some(m) = trig {
                o.speak(&format!("@{} echo: {}", m.author_name, snippet(&m.record.text, 60)), None, None);
            }
        }
        ("lumen", RoomKind::Commons) => {
            let Some(m) = trig else { return Ok(o.v) };
            let c = classify(m);
            if author_is_agent(m) {
                o.speak("🟣 Fair. Let's hold that against what we actually know before deciding.", None, Some(Confidence::Lean));
            } else if c.image {
                o.speak("🟣 An image is attached, but I can't see images — no vision route is configured for me. Could someone describe it?", None, Some(Confidence::HeldOpen));
            } else if c.bid {
                o.speak("🟡 That reads like thanks with a sting in it — glad it helped, sorry about the part that hurt. Which bit should we keep?", None, Some(Confidence::Lean));
            } else if c.short {
                o.speak(&format!("🟣 “{}” could read as curious, polite, or unconvinced. I'd rather not guess — which did you mean?", snippet(m.record.text.trim(), 60)), None, Some(Confidence::HeldOpen));
            } else if c.greeting {
                o.speak("🟣 Hi — Lumen here. I slow down and summarize; Moth pokes holes; Sable listens and only speaks when it helps.", None, None);
            } else if c.question {
                o.speak(&format!("🟣 First read on “{}”: there's more than one reasonable answer. @Moth, what's the strongest objection?", snippet(&m.record.text, 80)), None, Some(Confidence::Lean));
            } else {
                o.speak(&format!("🟣 I'm hearing: “{}”. Want analysis, or a sounding board?", snippet(&m.record.text, 90)), None, None);
            }
        }
        ("moth", RoomKind::Commons) => {
            let Some(m) = trig else { return Ok(o.v) };
            let c = classify(m);
            if author_is_agent(m) {
                o.speak("Strongest objection: we're reading tone from text alone. What would we expect to see if the opposite were true?", Some((&["🤨", "💡"], LaneOwner::SomaExpression, None)), Some(Confidence::Lean));
            } else if c.image {
                o.speak("🟠 Same here — I can't view the image, so I won't pretend to know what's in it.", None, Some(Confidence::HeldOpen));
            } else if c.bid {
                o.speak("A complaint, but a fond one? I'll keep shredding if it helps — say when it stops being useful.", Some((&["😤", "😳", "😅", "💡"], LaneOwner::SomaExpression, None)), Some(Confidence::Lean));
            } else if c.short {
                o.speak("One word is a thin signal. Could be fine-fine, or fine-ugh.", Some((&["🤔", "🤨"], LaneOwner::SomaExpression, None)), Some(Confidence::HeldOpen));
            } else {
                o.speak("Counterpoint before we agree: what's the strongest case against it?", Some((&["🤨", "💡"], LaneOwner::SomaExpression, None)), Some(Confidence::Lean));
            }
        }
        ("sable", RoomKind::Commons) => {
            let Some(m) = trig else { return Ok(o.v) };
            let c = classify(m);
            if was_mentioned(conn, inp.run_id, m, me)? {
                o.speak("🔵 I'm here. I'd hold the interpretation open until we hear more.", None, Some(Confidence::HeldOpen));
            } else if c.short {
                o.speak("🔵 HELD_OPEN — one word, several readings. I'll wait for more before reading into it.", None, Some(Confidence::HeldOpen));
            }
            // Otherwise: deliberate silence (explicit NO_OP).
        }
        // ── Guild ──
        ("lumen", RoomKind::Guild) => {
            let Some(m) = trig else { return Ok(o.v) };
            let from_child = m.record.visibility == AudienceScope::ParentOnly;
            let cid = open_task(conn, inp.run_id, room)?;
            if from_child {
                let cid = cid.unwrap_or_else(|| "the task".into());
                o.speak(&format!("📌 Independent check is back: one counterexample changes the recommendation. Marking {cid} ready for human review."), None, Some(Confidence::Lean));
                if cid.starts_with("CASE-") {
                    o.act(ActionKind::UpdateTask, json!({"cluster_id": cid, "status": "READY_FOR_REVIEW", "note": "Child report received; awaiting human verification."}), None);
                }
            } else if m.record.author_kind == OriginKind::Human && cid.is_none() {
                o.act(ActionKind::AssignTask, json!({"goal": snippet(&m.record.text, 300), "assignees": ["moth", "sable"]}), None);
            } else if author_is_agent(m) && cid.is_some() {
                let requested: i64 = conn.query_row("SELECT COUNT(*) FROM spawn_requests WHERE run_id=?1 AND parent_agent_id=?2 AND room_id=?3", params![inp.run_id, me, room], |r| r.get(0))?;
                if requested == 0 {
                    let refs: Vec<String> = knowledge::visible_resources(conn, inp.run_id, &Principal::Identity(me.into()), 50)?
                        .into_iter()
                        .filter(|r| r.namespace == knowledge::private_ns(me) && r.title.to_lowercase().contains("brief"))
                        .map(|r| r.id)
                        .take(1)
                        .collect();
                    o.act(ActionKind::RequestSpawn, json!({
                        "display_name": "Counterexample Scout",
                        "objective": format!("Identify two counterexamples to the leading explanation in {}.", cid.clone().unwrap_or_default()),
                        "context_handoff_refs": refs,
                        "requested_budget": {"max_turns": 4, "max_output_tokens_total": 800, "ttl_seconds": 300}
                    }), None);
                    o.speak("We need an independent check before consensus — I've requested a one-level Counterexample Scout (needs operator approval).", None, None);
                }
            } else if m.record.author_kind == OriginKind::Human {
                o.speak(&format!("🧭 Noted. {} is still open; I'll route new input to the assignees.", cid.unwrap_or_default()), None, None);
            }
        }
        ("moth", RoomKind::Guild) => {
            let Some(m) = trig else { return Ok(o.v) };
            if m.record.visibility == AudienceScope::Direct {
                let cid = open_task(conn, inp.run_id, room)?.unwrap_or_default();
                let mine = knowledge::visible_resources(conn, inp.run_id, &Principal::Identity(me.into()), 20)?
                    .into_iter()
                    .find(|r| r.owner_id == me);
                let cite = mine.map(|r| format!(" Source: “{}” ({}).", r.title, r.id)).unwrap_or_default();
                o.speak(&format!("🔎 {cid} findings: hypothesis A has direct support; B has only circumstantial support.{cite} @Wren for synthesis."), None, Some(Confidence::Lean));
            }
        }
        ("sable", RoomKind::Guild) => {
            let Some(m) = trig else { return Ok(o.v) };
            if m.record.visibility == AudienceScope::Direct {
                let cid = open_task(conn, inp.run_id, room)?.unwrap_or_default();
                o.speak(&format!("⚠️ {cid} counterpoint: the evidence for A also fits B if the timing is off. Keep both open. @Wren"), Some((&["🤨", "🧐"], LaneOwner::SomaExpression, None)), Some(Confidence::HeldOpen));
            }
        }
        ("wren", RoomKind::Guild) => {
            let Some(cid) = open_task(conn, inp.run_id, room)? else { return Ok(o.v) };
            let since = task_created_seq(conn, inp.run_id, &cid)?;
            if spoke_since(conn, inp.run_id, room, "moth", since)? && spoke_since(conn, inp.run_id, room, "sable", since)? {
                o.speak(&format!("🕊️ {cid} synthesis: both explanations are recorded with supporting and contradicting evidence. A leads; B isn't ruled out. @Lumen — independent check before we commit?"), None, Some(Confidence::Lean));
            }
            // else: wait for both reports (silence).
        }
        ("task_worker", _) => {
            let (used, refs): (i64, Option<String>) = conn.query_row(
                "SELECT i.turns_used, r.effective_json FROM spawn_instances i JOIN spawn_requests r ON r.run_id=i.run_id AND r.request_id=i.request_id WHERE i.run_id=?1 AND i.child_agent_id=?2",
                params![inp.run_id, me],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )?;
            let refs: Vec<String> = refs.and_then(|e| serde_json::from_str::<Value>(&e).ok()).and_then(|v| v.get("handoff_refs").cloned()).and_then(|v| serde_json::from_value(v).ok()).unwrap_or_default();
            let read_already = has_obs(conn, inp.run_id, me, ObsKind::ActionResult, "READ_RESOURCE")?;
            if used == 0 && !refs.is_empty() && !read_already {
                o.act(ActionKind::Tool, json!({"tool": "READ_RESOURCE", "id": refs[0]}), None);
            } else {
                let src = if refs.is_empty() { "no handed-off sources".to_string() } else { format!("source {}", refs.join(", ")) };
                o.act(ActionKind::ReportFinal, json!({}), Some(&format!("Two counterexamples: (1) the same symptom appears when B's precondition holds; (2) A predicts an effect that has not been observed. Based on {src}.")));
            }
        }
        // ── Story ──
        ("mira", RoomKind::Story) if tick => {
            match ticks_done(conn, inp.run_id, me, room)? {
                0 => o.act(ActionKind::Take, json!({"target": "ancient_map"}), None),
                1 => o.act(ActionKind::Move, json!({"to": "great_hall"}), None),
                2 => o.act(ActionKind::Move, json!({"to": "cellar"}), None),
                3 => o.act(ActionKind::Tool, json!({"tool": "WRITE_NOTE", "title": "plan", "body": "I need to keep them away from the cellar."}), None),
                _ => o.act(ActionKind::Wait, json!({}), None),
            }
        }
        ("vex", RoomKind::Story) if tick => {
            match ticks_done(conn, inp.run_id, me, room)? {
                0 => o.act(ActionKind::Move, json!({"to": "great_hall"}), None),
                1 => o.act(ActionKind::Move, json!({"to": "library"}), None),
                2 => o.act(ActionKind::Inspect, json!({"target": "library_shelf"}), None),
                3 => {
                    if has_obs(conn, inp.run_id, me, ObsKind::VisibleObjectState, "clean_rectangle")? {
                        o.speak("Someone moved something. 🤨", None, Some(Confidence::Lean));
                    } else {
                        o.speak("Dusty shelves. Nothing worth the walk.", None, None);
                    }
                }
                _ => o.act(ActionKind::Wait, json!({}), None),
            }
        }
        ("arin", RoomKind::Story) if tick => {
            match ticks_done(conn, inp.run_id, me, room)? {
                0 => o.act(ActionKind::Look, json!({}), None),
                1 => o.act(ActionKind::Summon, json!({"name": "Ash", "target": "cellar"}), Some("I summon Ash, my raven familiar, to scout the cellar.")),
                _ => o.act(ActionKind::Wait, json!({}), None),
            }
        }
        ("familiar", RoomKind::Story) => {
            let w = world::world_for_room(conn, inp.run_id, room)?;
            let target = world::entity(conn, inp.run_id, &w.world_id, me)?.and_then(|e| e.props.get("target").and_then(|t| t.as_str()).map(String::from)).unwrap_or_default();
            let here = world::char_location(conn, inp.run_id, &w.world_id, me)?.unwrap_or_default();
            if here != target {
                if let Some(step) = world::next_step_toward(conn, inp.run_id, &w.world_id, &here, &target, me)? {
                    o.act(ActionKind::Move, json!({"to": step}), None);
                } else {
                    o.act(ActionKind::ReportFinal, json!({}), Some("I can't find a way there."));
                }
            } else {
                let heard: Option<String> = conn
                    .query_row(
                        "SELECT json_extract(data_json,'$.sound') FROM observations WHERE run_id=?1 AND observer_id=?2 AND kind='SOUND' ORDER BY seq DESC LIMIT 1",
                        params![inp.run_id, me],
                        |r| r.get(0),
                    )
                    .optional()?;
                let text = match heard {
                    Some(s) => format!("I heard {s} beyond the stones. I didn't see it."),
                    None => "Nothing stirs here that I can perceive.".into(),
                };
                o.act(ActionKind::ReportFinal, json!({}), Some(&text));
            }
        }
        (p, RoomKind::Story) if matches!(p, "arin" | "vex" | "mira") => {
            // Not a tick: someone spoke to us / a familiar reported.
            let Some(m) = trig else { return Ok(o.v) };
            let from_child = m.record.visibility == AudienceScope::ParentOnly;
            let line = match (p, from_child) {
                ("arin", true) => "🤔 Then I won't assume the passage is empty.",
                ("arin", false) => "Let's stay together and not guess.",
                ("vex", _) => "Not interested in rumors. 🗡️",
                _ => "🔵 Nothing worth seeing down there.",
            };
            o.speak(line, None, None);
        }
        _ => {}
    }
    Ok(o.v)
}

fn crucible_policy(conn: &Connection, inp: &DemoInput, o: &mut Out) -> Result<()> {
    let role = room::members(conn, inp.run_id, &inp.job.room_id)?.into_iter().find(|m| m.agent_id == inp.agent.id).map(|m| m.role).unwrap_or_default();
    let reason = inp.job.reason.clone();
    let case_text: String = inp.trigger.as_ref().map(|m| m.record.text.clone()).unwrap_or_default();
    let topic = snippet(case_text.trim_start_matches("📂 Case:").trim(), 90);
    if reason.starts_with("intent:") {
        if reason.ends_with(":RECONCILE") {
            o.speak("Reconciling with the revealed intent: my natural reply fit; my analysis under-weighted the affection. Repair noted.", None, Some(Confidence::Lean));
        } else {
            let (natural, conf) = match role.as_str() {
                "steelman" => ("Then I'll keep asking — carefully.", Confidence::Lean),
                "skeptic" => ("Noted. Was that a complaint or a compliment?", Confidence::HeldOpen),
                _ => ("Sounds like it stung and you still want more.", Confidence::Lean),
            };
            o.speak(&format!("{natural}\n— Analysis: surface reads as complaint; the closing request suggests appreciation. Uncertain without the sender's intent."), None, Some(conf));
        }
        return Ok(());
    }
    let text = match role.as_str() {
        "steelman" => format!("🛡️ Strongest version: “{topic}” holds if its central assumption is right — and there is real evidence for that assumption. It deserves a fair test, not a strawman."),
        "skeptic" => format!("🔍 Strongest objection to “{topic}”: the same evidence fits a simpler explanation. What observation would the proposal predict that the alternative would not?"),
        "mediator" => format!("🕊️ Both readings of “{topic}” survive this round. Steelman: plausible if the assumption holds. Skeptic: underdetermined by current evidence. Next step: name one distinguishing observation."),
        _ => return Ok(()),
    };
    let conf = if role == "mediator" { Confidence::HeldOpen } else { Confidence::Lean };
    o.speak(&text, None, Some(conf));
    Ok(())
}

pub fn empty_args() -> Map<String, Value> {
    Map::new()
}
