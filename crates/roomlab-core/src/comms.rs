//! COMMS router (§13.3). A message is delivered once to each eligible member; delivery
//! writes an observation row per recipient. Agents never read the `messages` table
//! directly — the context builder only joins messages through the reader's observations.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth;
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::new_id;
use crate::room;

pub const MAX_TEXT_CHARS: usize = 4000;

#[derive(Clone, Debug, Default)]
pub struct PostSpec {
    pub room_id: String,
    pub author_id: String,
    pub text: String,
    pub visibility: Option<AudienceScope>,
    pub recipients: Vec<String>,
    pub attachments: Vec<String>,
    pub reaction_lane: Option<ReactionLane>,
    pub confidence: Option<Confidence>,
    pub is_demo: bool,
    pub model_id: Option<String>,
    pub chain_depth: i64,
    pub parents: Vec<String>,
    /// Trusted internal origins (WORLD/SYSTEM/TOOL/NARRATOR) that are not member identities.
    pub system_origin: Option<OriginKind>,
    /// Restrict audience to these identities (Story: same-location listeners).
    pub audience_limit: Option<Vec<String>>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Posted {
    pub message: MessageRecord,
    pub delivered_to: Vec<String>,
    pub event_id: String,
    pub seq: i64,
}

/// Remove control characters except newline/tab; keep all Unicode (ZWJ, VS16, RTL marks).
pub fn sanitize_text(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .trim_end()
        .to_string()
}

pub fn post(conn: &Connection, run_id: &str, spec: PostSpec) -> Result<Posted> {
    let text = sanitize_text(&spec.text);
    if text.chars().count() > MAX_TEXT_CHARS {
        return reject(ReasonCode::InvalidSchema, format!("message exceeds {MAX_TEXT_CHARS} characters"));
    }
    if text.trim().is_empty() && spec.attachments.is_empty() {
        return reject(ReasonCode::InvalidSchema, "empty message");
    }
    let settings = room::room_settings(conn, run_id, &spec.room_id)?;
    let members = room::member_ids(conn, run_id, &spec.room_id)?;

    let (author_kind, mut visibility, mut recipients) = match spec.system_origin {
        Some(k) => (k, spec.visibility.unwrap_or(AudienceScope::RoomMembers), spec.recipients.clone()),
        None => {
            let a = auth::require_active(conn, run_id, &spec.author_id)?;
            let m = auth::require_member(conn, run_id, &spec.room_id, &spec.author_id)?;
            if settings.read_only && a.kind != AgentKind::Human {
                return reject(ReasonCode::CapabilityDenied, "room is read-only");
            }
            if !m.can_post || m.post_scope == "NONE" {
                return reject(ReasonCode::CapabilityDenied, "identity may not post in this room");
            }
            let kind = match a.kind {
                AgentKind::Human => OriginKind::Human,
                AgentKind::Narrator => OriginKind::Narrator,
                _ => OriginKind::Agent,
            };
            let mut vis = spec.visibility.unwrap_or(AudienceScope::RoomMembers);
            let mut rec = spec.recipients.clone();
            if m.post_scope == "PARENT_ONLY" {
                // Task children speak only to their parent unless granted (§12.3).
                let parent = a.parent_agent_id.clone().ok_or_else(|| CoreError::Invalid("child without parent".into()))?;
                vis = AudienceScope::ParentOnly;
                rec = vec![parent];
            }
            (kind, vis, rec)
        }
    };
    if visibility == AudienceScope::Public {
        visibility = AudienceScope::RoomMembers;
    }

    // Audience resolution: intent (recipient_ids) and delivery outcome differ.
    let mut audience: Vec<String> = match visibility {
        AudienceScope::RoomMembers => members.clone(),
        AudienceScope::Direct | AudienceScope::ParentOnly => {
            recipients.retain(|r| r != &spec.author_id);
            let mut v: Vec<String> = recipients.iter().filter(|r| members.contains(r)).cloned().collect();
            if members.contains(&spec.author_id) {
                v.push(spec.author_id.clone());
            }
            v
        }
        AudienceScope::Location => spec.audience_limit.clone().unwrap_or_default().into_iter().filter(|r| members.contains(r)).collect(),
        AudienceScope::ActorOnly => vec![spec.author_id.clone()].into_iter().filter(|r| members.contains(r)).collect(),
        AudienceScope::OperatorOnly => vec![],
        AudienceScope::Public => members.clone(),
    };
    if let Some(limit) = &spec.audience_limit {
        if visibility != AudienceScope::Location {
            audience.retain(|a| limit.contains(a) || a == &spec.author_id);
        }
    }
    // Archived/dormant identities do not receive new observations? Dormant ones still observe
    // (they wake with history); archived identities are excluded.
    let mut delivered = vec![];
    for id in audience {
        if delivered.contains(&id) {
            continue;
        }
        if let Some(a) = auth::load_agent(conn, run_id, &id)? {
            if a.status != AgentStatus::Archived {
                delivered.push(id);
            }
        }
    }
    if recipients.is_empty() {
        recipients = delivered.iter().filter(|d| *d != &spec.author_id).cloned().collect();
    }

    let message_id = new_id("msg");
    let modality = match (text.trim().is_empty(), spec.attachments.is_empty()) {
        (_, true) => Modality::Text,
        (true, false) => Modality::Image,
        (false, false) => Modality::Mixed,
    };
    let channel_id = if author_kind == OriginKind::Narrator { format!("{}:narrator", spec.room_id) } else { format!("{}:public", spec.room_id) };
    let ev = events::append(
        conn,
        run_id,
        NewEvent::new(EventType::MessageDelivered, author_kind, &spec.author_id, visibility, json!({
            "message_id": message_id, "delivered_count": delivered.len(), "is_demo": spec.is_demo, "model_id": spec.model_id
        }))
        .room(&spec.room_id)
        .parents(spec.parents.clone()),
    )?;
    let rec = MessageRecord {
        message_id: message_id.clone(),
        room_id: spec.room_id.clone(),
        channel_id,
        author_id: spec.author_id.clone(),
        author_kind,
        recipient_ids: recipients.clone(),
        visibility,
        text: text.clone(),
        attachments: spec.attachments.clone(),
        modality,
        source_event_id: ev.event_id.clone(),
        render_status: RenderStatus::Final,
        reaction_lane: spec.reaction_lane.clone(),
        confidence: spec.confidence,
        is_demo: spec.is_demo,
        model_id: spec.model_id.clone(),
        seq: ev.seq,
    };
    conn.execute(
        "INSERT INTO messages(run_id, message_id, room_id, channel_id, author_id, author_kind, recipient_ids_json, visibility, text, attachments_json, modality, source_event_id, render_status, reaction_lane_json, confidence, is_demo, model_id, chain_depth, seq)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)",
        params![
            run_id, rec.message_id, rec.room_id, rec.channel_id, rec.author_id, rec.author_kind.as_str(),
            serde_json::to_string(&rec.recipient_ids)?, rec.visibility.as_str(), rec.text, serde_json::to_string(&rec.attachments)?,
            rec.modality.as_str(), rec.source_event_id, rec.render_status.as_str(),
            rec.reaction_lane.as_ref().map(serde_json::to_string).transpose()?, rec.confidence.map(|c| c.as_str()),
            rec.is_demo as i64, rec.model_id, spec.chain_depth, rec.seq
        ],
    )?;
    let sim = events::sim_time(conn, run_id)?;
    let channel = match visibility {
        AudienceScope::Direct | AudienceScope::ParentOnly => ObsChannel::Direct,
        AudienceScope::Location => ObsChannel::WorldLocal,
        _ => ObsChannel::RoomChat,
    };
    for d in &delivered {
        record_observation(conn, run_id, d, Some(&spec.room_id), &ev.event_id, channel, ObsKind::Message, json!({"message_id": message_id}), ObsStatus::Observed, "comms_router_v1", sim, None, ev.seq)?;
    }
    Ok(Posted { message: rec, delivered_to: delivered, event_id: ev.event_id, seq: ev.seq })
}

#[allow(clippy::too_many_arguments)]
pub fn record_observation(
    conn: &Connection, run_id: &str, observer: &str, room_id: Option<&str>, source_event_id: &str, channel: ObsChannel,
    kind: ObsKind, data: serde_json::Value, status: ObsStatus, provenance: &str, sim_ms: i64, expires: Option<i64>, seq: i64,
) -> Result<String> {
    let id = new_id("obs");
    conn.execute(
        "INSERT INTO observations(run_id, observation_id, observer_id, room_id, source_event_id, channel, kind, data_json, status, provenance, observed_at_sim_ms, expires_at_sim_ms, seq)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
        params![run_id, id, observer, room_id, source_event_id, channel.as_str(), kind.as_str(), serde_json::to_string(&data)?, status.as_str(), provenance, sim_ms, expires, seq],
    )?;
    Ok(id)
}

/// Resolve `@Name` / `@id` mentions against room members (display names may contain spaces only via id).
pub fn parse_mentions(conn: &Connection, run_id: &str, room_id: &str, text: &str) -> Result<Vec<String>> {
    let members = room::members(conn, run_id, room_id)?;
    let lower = text.to_lowercase();
    let mut out = vec![];
    for m in members {
        let by_name = format!("@{}", m.display_name.to_lowercase());
        let by_id = format!("@{}", m.agent_id.to_lowercase());
        let hit = |needle: &str| {
            lower.match_indices(needle).any(|(i, _)| {
                let end = i + needle.len();
                lower[end..].chars().next().map(|c| !c.is_alphanumeric() && c != '_').unwrap_or(true)
            })
        };
        if hit(&by_name) || hit(&by_id) {
            out.push(m.agent_id);
        }
    }
    if lower.contains("@everyone") || lower.contains("@all") {
        out.push("*".into());
    }
    Ok(out)
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct MessageView {
    #[serde(flatten)]
    pub record: MessageRecord,
    pub author_name: String,
    pub author_icon: String,
    pub recipient_names: Vec<String>,
    /// Not visible to every room member.
    pub private: bool,
    pub chain_depth: i64,
}

fn load_messages(conn: &Connection, run_id: &str, sql: &str, p: &[&dyn rusqlite::ToSql]) -> Result<Vec<MessageView>> {
    let mut st = conn.prepare(sql)?;
    let rows = st
        .query_map(p, |r| {
            Ok((
                r.get::<_, String>("message_id")?, r.get::<_, String>("room_id")?, r.get::<_, String>("channel_id")?,
                r.get::<_, String>("author_id")?, r.get::<_, String>("author_kind")?, r.get::<_, String>("recipient_ids_json")?,
                r.get::<_, String>("visibility")?, r.get::<_, String>("text")?, r.get::<_, String>("attachments_json")?,
                r.get::<_, String>("modality")?, r.get::<_, String>("source_event_id")?, r.get::<_, String>("render_status")?,
                r.get::<_, Option<String>>("reaction_lane_json")?, r.get::<_, Option<String>>("confidence")?,
                r.get::<_, i64>("is_demo")?, r.get::<_, Option<String>>("model_id")?, r.get::<_, i64>("seq")?,
                r.get::<_, i64>("chain_depth")?, r.get::<_, Option<String>>("author_name")?, r.get::<_, Option<String>>("author_icon")?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = vec![];
    for (mid, room, ch, aid, ak, rj, vis, text, aj, modality, sev, rs, lane, conf, demo, model, seq, depth, an, ai) in rows {
        let recipient_ids: Vec<String> = serde_json::from_str(&rj)?;
        let visibility: AudienceScope = parse_enum(&vis)?;
        let author_kind: OriginKind = parse_enum(&ak)?;
        let names = recipient_ids.iter().map(|r| crate::agent::display_name(conn, run_id, r)).collect();
        out.push(MessageView {
            author_name: an.unwrap_or_else(|| match author_kind {
                OriginKind::World => "World".into(),
                OriginKind::System => "System".into(),
                OriginKind::Tool => "Tool".into(),
                OriginKind::Narrator => "Narrator".into(),
                _ => aid.clone(),
            }),
            author_icon: ai.unwrap_or_default(),
            recipient_names: names,
            private: !matches!(visibility, AudienceScope::RoomMembers | AudienceScope::Public),
            chain_depth: depth,
            record: MessageRecord {
                message_id: mid,
                room_id: room,
                channel_id: ch,
                author_id: aid,
                author_kind,
                recipient_ids,
                visibility,
                text,
                attachments: serde_json::from_str(&aj)?,
                modality: parse_enum(&modality)?,
                source_event_id: sev,
                render_status: parse_enum(&rs)?,
                reaction_lane: lane.map(|l| serde_json::from_str(&l)).transpose()?,
                confidence: conf.map(|c| parse_enum(&c)).transpose()?,
                is_demo: demo != 0,
                model_id: model,
                seq,
            },
        });
    }
    Ok(out)
}

/// Messages a viewer may see. `viewer = None` means operator view (everything, private flagged).
pub fn list_messages(conn: &Connection, run_id: &str, room_id: &str, viewer: Option<&str>, limit: i64) -> Result<Vec<MessageView>> {
    let limit = limit.clamp(1, 2000);
    let mut v = match viewer {
        None => load_messages(
            conn,
            run_id,
            "SELECT m.*, a.display_name AS author_name, a.icon AS author_icon FROM messages m
             LEFT JOIN agents a ON a.run_id=m.run_id AND a.id=m.author_id
             WHERE m.run_id=?1 AND m.room_id=?2 ORDER BY m.seq DESC LIMIT ?3",
            &[&run_id, &room_id, &limit],
        )?,
        Some(viewer) => load_messages(
            conn,
            run_id,
            "SELECT m.*, a.display_name AS author_name, a.icon AS author_icon FROM messages m
             LEFT JOIN agents a ON a.run_id=m.run_id AND a.id=m.author_id
             WHERE m.run_id=?1 AND m.room_id=?2 AND EXISTS (
               SELECT 1 FROM observations o WHERE o.run_id=m.run_id AND o.observer_id=?3 AND o.kind='MESSAGE'
               AND json_extract(o.data_json,'$.message_id') = m.message_id)
             ORDER BY m.seq DESC LIMIT ?4",
            &[&run_id, &room_id, &viewer, &limit],
        )?,
    };
    v.reverse();
    Ok(v)
}

pub fn get_message(conn: &Connection, run_id: &str, message_id: &str) -> Result<MessageView> {
    load_messages(
        conn,
        run_id,
        "SELECT m.*, a.display_name AS author_name, a.icon AS author_icon FROM messages m
         LEFT JOIN agents a ON a.run_id=m.run_id AND a.id=m.author_id WHERE m.run_id=?1 AND m.message_id=?2",
        &[&run_id, &message_id],
    )?
    .pop()
    .ok_or_else(|| CoreError::NotFound(format!("message {message_id}")))
}
