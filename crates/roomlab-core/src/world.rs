//! World Authority (§6.1). Trusted, deterministic, seeded. Owns the room graph, objects,
//! inventory, hidden exits, observation checks and summons. Characters *propose* actions;
//! this module decides eligibility and outcomes. Narrators only render validated packets.

use std::collections::{HashSet, VecDeque};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::auth::AgentRow;
use crate::comms::{self, PostSpec, Posted};
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::d20;
use crate::knowledge::{self, NewResource};
use crate::narrative::{self, NarrationPacket};

#[derive(Clone, Debug, Serialize)]
pub struct WorldRef {
    pub world_id: String,
    pub room_id: String,
    pub seed: i64,
    pub revision: i64,
    pub capacity: i64,
    pub resolution: NarrativeResolution,
    pub genre: String,
    pub title: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entity {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub location: Option<String>,
    pub state: String,
    pub visibility: String,
    pub props: Value,
    pub rev: i64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExitView {
    pub to: String,
    pub name: String,
    pub locked: bool,
    pub secret: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ObjView {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LocationView {
    pub location_id: String,
    pub name: String,
    pub ambient: String,
    pub description: String,
    pub exits: Vec<ExitView>,
    pub objects: Vec<ObjView>,
    pub characters: Vec<String>,
    pub inventory: Vec<ObjView>,
}

pub fn world_id_for_room(room_id: &str) -> String {
    format!("w_{room_id}")
}

pub fn world_for_room(conn: &Connection, run_id: &str, room_id: &str) -> Result<WorldRef> {
    let w = conn
        .query_row(
            "SELECT world_id, room_id, seed, revision, inventory_capacity, narrative_resolution, genre, title FROM worlds WHERE run_id=?1 AND room_id=?2",
            params![run_id, room_id],
            |r| {
                Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, i64>(3)?, r.get::<_, i64>(4)?, r.get::<_, String>(5)?, r.get::<_, String>(6)?, r.get::<_, String>(7)?))
            },
        )
        .optional()?;
    match w {
        Some((world_id, room_id, seed, revision, capacity, res, genre, title)) => Ok(WorldRef { world_id, room_id, seed, revision, capacity, resolution: parse_enum(&res)?, genre, title }),
        None => reject(ReasonCode::WorldRuleDenied, "this room has no world; world actions are only valid in Story rooms"),
    }
}

pub fn validate_spec(spec: &WorldSpec) -> Result<()> {
    let bad = |m: String| Err(CoreError::Invalid(format!("world spec: {m}")));
    if spec.schema_version != SCHEMA_VERSION {
        return bad("unsupported schema_version".into());
    }
    let locs: HashSet<&str> = spec.locations.iter().map(|l| l.id.as_str()).collect();
    if locs.len() != spec.locations.len() {
        return bad("duplicate location id".into());
    }
    if locs.len() < 2 {
        return bad("at least two locations required".into());
    }
    if !locs.contains(spec.start_location.as_str()) {
        return bad("start_location not found".into());
    }
    for l in &spec.locations {
        for e in &l.exits {
            if !locs.contains(e.as_str()) {
                return bad(format!("exit {} -> {} references unknown location", l.id, e));
            }
        }
    }
    let mut ids: HashSet<&str> = HashSet::new();
    for o in &spec.objects {
        if !ids.insert(o.id.as_str()) {
            return bad(format!("duplicate object {}", o.id));
        }
        if !o.location.is_empty() && !locs.contains(o.location.as_str()) {
            return bad(format!("object {} in unknown location", o.id));
        }
        if !matches!(o.visibility.as_str(), "VISIBLE" | "NOTICEABLE_IF_INSPECTED" | "HIDDEN") {
            return bad(format!("object {} has invalid visibility", o.id));
        }
    }
    for h in &spec.hidden_edges {
        if !locs.contains(h.from.as_str()) || !locs.contains(h.to.as_str()) {
            return bad("hidden edge references unknown location".into());
        }
    }
    for c in &spec.characters {
        if !locs.contains(c.location.as_str()) {
            return bad(format!("character {} in unknown location", c.agent_id));
        }
        for i in &c.inventory {
            if !ids.contains(i.as_str()) {
                return bad(format!("inventory item {i} not an object"));
            }
        }
    }
    Ok(())
}

pub fn load_world(conn: &Connection, run_id: &str, room_id: &str, spec: &WorldSpec, resolution: NarrativeResolution) -> Result<String> {
    validate_spec(spec)?;
    let wid = world_id_for_room(room_id);
    conn.execute(
        "INSERT INTO worlds(run_id, world_id, room_id, seed, revision, narrative_resolution, genre, title, spec_json, inventory_capacity) VALUES (?1,?2,?3,?4,1,?5,?6,?7,?8,?9)",
        params![run_id, wid, room_id, spec.seed, resolution.as_str(), spec.genre, spec.title, serde_json::to_string(spec)?, spec.inventory_capacity],
    )?;
    for l in &spec.locations {
        conn.execute(
            "INSERT INTO locations(run_id, world_id, id, name, ambient, description) VALUES (?1,?2,?3,?4,?5,?6)",
            params![run_id, wid, l.id, l.name, l.ambient, l.description],
        )?;
        for e in &l.exits {
            for (a, b) in [(&l.id, e), (e, &l.id)] {
                conn.execute(
                    "INSERT OR IGNORE INTO world_edges(run_id, world_id, from_loc, to_loc, hidden) VALUES (?1,?2,?3,?4,0)",
                    params![run_id, wid, a, b],
                )?;
            }
        }
    }
    for h in &spec.hidden_edges {
        for (a, b) in [(&h.from, &h.to), (&h.to, &h.from)] {
            conn.execute(
                "INSERT OR REPLACE INTO world_edges(run_id, world_id, from_loc, to_loc, hidden, discover_dc, discovered_by_json, door_entity_id) VALUES (?1,?2,?3,?4,1,?5,?6,?7)",
                params![run_id, wid, a, b, h.discover_dc, serde_json::to_string(&h.known_by)?, h.door_object_id],
            )?;
        }
    }
    for o in &spec.objects {
        let mut noticed: Vec<String> = o.known_by.clone();
        for h in &spec.hidden_edges {
            if h.door_object_id.as_deref() == Some(o.id.as_str()) {
                noticed.extend(h.known_by.iter().cloned());
            }
        }
        let props = json!({
            "description": o.description, "takeable": o.takeable, "unlocks": o.unlocks,
            "notice_dc": o.notice_dc, "trace_on_take": o.trace_on_take, "noticed_by": noticed, "cue": Value::Null,
            "found_with": o.found_with
        });
        conn.execute(
            "INSERT INTO world_entities(run_id, world_id, id, name, kind, location_id, state, visibility, props_json, rev) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,0)",
            params![run_id, wid, o.id, o.name, o.kind, if o.location.is_empty() { None } else { Some(o.location.clone()) }, o.state, o.visibility, props.to_string()],
        )?;
    }
    let seq = events::current_seq(conn, run_id)?;
    for c in &spec.characters {
        let name = crate::agent::display_name(conn, run_id, &c.agent_id);
        let props = json!({"abilities": c.abilities, "spell_slots": c.spell_slots, "perception": c.perception});
        conn.execute(
            "INSERT INTO world_entities(run_id, world_id, id, name, kind, location_id, state, visibility, props_json, rev) VALUES (?1,?2,?3,?4,'CHARACTER',?5,'','VISIBLE',?6,0)",
            params![run_id, wid, c.agent_id, name, c.location, props.to_string()],
        )?;
        for i in &c.inventory {
            conn.execute("UPDATE world_entities SET location_id=NULL WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, wid, i])?;
            conn.execute(
                "INSERT INTO inventories(run_id, world_id, holder_id, entity_id, acquired_seq) VALUES (?1,?2,?3,?4,?5)",
                params![run_id, wid, c.agent_id, i, seq],
            )?;
        }
    }
    for (n, s) in spec.sounds.iter().enumerate() {
        conn.execute(
            "INSERT INTO world_entities(run_id, world_id, id, name, kind, location_id, state, visibility, props_json, rev) VALUES (?1,?2,?3,'sound','SOUND',?4,'active','HIDDEN',?5,0)",
            params![run_id, wid, format!("sound_{n}"), s.location, json!({"description": s.description, "hidden_source": s.hidden_source}).to_string()],
        )?;
    }
    for f in &spec.secret_facts {
        knowledge::add_resource(conn, run_id, NewResource {
            namespace: &format!("world/{wid}/truth"),
            owner_id: "world",
            title: &format!("Secret fact {}", f.id),
            body: &f.claim,
            kind: KnowledgeKind::WorldTruth,
            visibility: ResourceVisibility::OperatorOnly,
            source_id: &format!("world:{}", f.id),
        })?;
        if let Some(agent) = f.owner_scope.strip_prefix("agent/").and_then(|r| r.strip_suffix("/private")) {
            knowledge::add_resource(conn, run_id, NewResource {
                namespace: &knowledge::private_ns(agent),
                owner_id: agent,
                title: "Private knowledge",
                body: &f.claim,
                kind: KnowledgeKind::Belief,
                visibility: ResourceVisibility::Private,
                source_id: &format!("world:{}", f.id),
            })?;
        }
    }
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::WorldStateChanged, OriginKind::World, "world", AudienceScope::OperatorOnly, json!({"loaded": spec.world_id, "world_id": wid})).room(room_id),
    )?;
    Ok(wid)
}

fn row_entity(r: &rusqlite::Row) -> rusqlite::Result<Entity> {
    let props: String = r.get("props_json")?;
    Ok(Entity {
        id: r.get("id")?,
        name: r.get("name")?,
        kind: r.get("kind")?,
        location: r.get("location_id")?,
        state: r.get("state")?,
        visibility: r.get("visibility")?,
        props: serde_json::from_str(&props).unwrap_or(Value::Null),
        rev: r.get("rev")?,
    })
}

pub fn entity(conn: &Connection, run_id: &str, wid: &str, id: &str) -> Result<Option<Entity>> {
    Ok(conn
        .query_row("SELECT * FROM world_entities WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, wid, id], row_entity)
        .optional()?)
}

pub fn all_entities(conn: &Connection, run_id: &str, wid: &str) -> Result<Vec<Entity>> {
    let mut st = conn.prepare("SELECT * FROM world_entities WHERE run_id=?1 AND world_id=?2 ORDER BY kind, id")?;
    let v = st.query_map(params![run_id, wid], row_entity)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

fn entities_at(conn: &Connection, run_id: &str, wid: &str, loc: &str) -> Result<Vec<Entity>> {
    let mut st = conn.prepare("SELECT * FROM world_entities WHERE run_id=?1 AND world_id=?2 AND location_id=?3 ORDER BY id")?;
    let v = st.query_map(params![run_id, wid, loc], row_entity)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn inventory(conn: &Connection, run_id: &str, wid: &str, holder: &str) -> Result<Vec<Entity>> {
    let mut st = conn.prepare(
        "SELECT e.* FROM inventories i JOIN world_entities e ON e.run_id=i.run_id AND e.world_id=i.world_id AND e.id=i.entity_id
         WHERE i.run_id=?1 AND i.world_id=?2 AND i.holder_id=?3 ORDER BY i.acquired_seq",
    )?;
    let v = st.query_map(params![run_id, wid, holder], row_entity)?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

pub fn holder_of(conn: &Connection, run_id: &str, wid: &str, entity_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row("SELECT holder_id FROM inventories WHERE run_id=?1 AND world_id=?2 AND entity_id=?3", params![run_id, wid, entity_id], |r| r.get(0))
        .optional()?)
}

pub fn char_location(conn: &Connection, run_id: &str, wid: &str, agent_id: &str) -> Result<Option<String>> {
    Ok(conn
        .query_row(
            "SELECT location_id FROM world_entities WHERE run_id=?1 AND world_id=?2 AND id=?3 AND kind IN ('CHARACTER','FAMILIAR')",
            params![run_id, wid, agent_id],
            |r| r.get(0),
        )
        .optional()?
        .flatten())
}

/// Characters (and familiars) present at a location — the perception audience.
pub fn listeners_at(conn: &Connection, run_id: &str, wid: &str, loc: &str) -> Result<Vec<String>> {
    let mut st = conn.prepare("SELECT id FROM world_entities WHERE run_id=?1 AND world_id=?2 AND location_id=?3 AND kind IN ('CHARACTER','FAMILIAR') ORDER BY id")?;
    let v = st.query_map(params![run_id, wid, loc], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(v)
}

fn loc_row(conn: &Connection, run_id: &str, wid: &str, loc: &str) -> Result<(String, String, String)> {
    Ok(conn.query_row(
        "SELECT name, ambient, description FROM locations WHERE run_id=?1 AND world_id=?2 AND id=?3",
        params![run_id, wid, loc],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?)
}

fn noticed_by(e: &Entity) -> Vec<String> {
    e.props.get("noticed_by").and_then(|v| serde_json::from_value(v.clone()).ok()).unwrap_or_default()
}

fn visible_to(e: &Entity, viewer: &str) -> bool {
    match e.visibility.as_str() {
        "VISIBLE" => true,
        _ => noticed_by(e).iter().any(|n| n == viewer),
    }
}

pub fn exits_for(conn: &Connection, run_id: &str, wid: &str, loc: &str, viewer: &str) -> Result<Vec<ExitView>> {
    let mut st = conn.prepare(
        "SELECT e.to_loc, l.name, e.hidden, e.discovered_by_json, e.door_entity_id FROM world_edges e JOIN locations l ON l.run_id=e.run_id AND l.world_id=e.world_id AND l.id=e.to_loc
         WHERE e.run_id=?1 AND e.world_id=?2 AND e.from_loc=?3 ORDER BY l.name",
    )?;
    let rows = st
        .query_map(params![run_id, wid, loc], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, i64>(2)?, r.get::<_, String>(3)?, r.get::<_, Option<String>>(4)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut out = vec![];
    for (to, name, hidden, disc, door) in rows {
        let discovered: Vec<String> = serde_json::from_str(&disc).unwrap_or_default();
        if hidden != 0 && !discovered.iter().any(|d| d == viewer) {
            continue;
        }
        let locked = match door {
            Some(d) => entity(conn, run_id, wid, &d)?.map(|e| e.state == "LOCKED").unwrap_or(false),
            None => false,
        };
        out.push(ExitView { to, name, locked, secret: hidden != 0 });
    }
    Ok(out)
}

pub fn view_for(conn: &Connection, run_id: &str, wid: &str, agent_id: &str) -> Result<LocationView> {
    let loc = char_location(conn, run_id, wid, agent_id)?.ok_or_else(|| CoreError::Rejected { code: ReasonCode::NotVisible, message: "not present in this world".into() })?;
    let (name, ambient, description) = loc_row(conn, run_id, wid, &loc)?;
    let ents = entities_at(conn, run_id, wid, &loc)?;
    let objects = ents
        .iter()
        .filter(|e| !matches!(e.kind.as_str(), "CHARACTER" | "FAMILIAR" | "SOUND") && visible_to(e, agent_id))
        .map(|e| ObjView { id: e.id.clone(), name: e.name.clone(), kind: e.kind.clone(), state: public_state(e) })
        .collect();
    let characters = ents.iter().filter(|e| matches!(e.kind.as_str(), "CHARACTER" | "FAMILIAR") && e.id != agent_id).map(|e| e.name.clone()).collect();
    let inv = inventory(conn, run_id, wid, agent_id)?.into_iter().map(|e| ObjView { id: e.id.clone(), name: e.name.clone(), kind: e.kind.clone(), state: public_state(&e) }).collect();
    Ok(LocationView { exits: exits_for(conn, run_id, wid, &loc, agent_id)?, location_id: loc, name, ambient, description, objects, characters, inventory: inv })
}

/// Perceivable state only (features' traces are revealed by checks, not by listing).
fn public_state(e: &Entity) -> String {
    match e.kind.as_str() {
        "DOOR" => e.state.to_lowercase(),
        _ => String::new(),
    }
}

fn resolve_location(conn: &Connection, run_id: &str, wid: &str, s: &str) -> Result<Option<String>> {
    let s = s.trim().to_lowercase();
    Ok(conn
        .query_row(
            "SELECT id FROM locations WHERE run_id=?1 AND world_id=?2 AND (lower(id)=?3 OR lower(name)=?3 OR lower(replace(name,' ','_'))=?3)",
            params![run_id, wid, s],
            |r| r.get(0),
        )
        .optional()?)
}

/// Resolve a target among entities the actor can currently perceive (location or inventory).
fn resolve_target(conn: &Connection, run_id: &str, wid: &str, loc: &str, actor: &str, target: &str) -> Result<Entity> {
    let t = target.trim().to_lowercase();
    let mut cands = entities_at(conn, run_id, wid, loc)?;
    cands.extend(inventory(conn, run_id, wid, actor)?);
    for e in cands {
        if matches!(e.kind.as_str(), "SOUND") {
            continue;
        }
        if e.id.to_lowercase() == t || e.name.to_lowercase() == t || e.name.to_lowercase().replace(' ', "_") == t {
            let held = holder_of(conn, run_id, wid, &e.id)?.as_deref() == Some(actor);
            if held || visible_to(&e, actor) {
                return Ok(e);
            }
            return reject(ReasonCode::NotVisible, format!("you cannot perceive '{target}' here"));
        }
    }
    reject(ReasonCode::TargetUnknown, format!("nothing called '{target}' is here"))
}

fn arg<'a>(req: &'a ActionRequest, k: &str) -> Option<&'a str> {
    req.args.get(k).and_then(|v| v.as_str())
}

fn bump(conn: &Connection, run_id: &str, wid: &str) -> Result<i64> {
    Ok(conn.query_row("UPDATE worlds SET revision=revision+1 WHERE run_id=?1 AND world_id=?2 RETURNING revision", params![run_id, wid], |r| r.get(0))?)
}

fn set_props(conn: &Connection, run_id: &str, wid: &str, e: &Entity, props: &Value, seq: i64) -> Result<()> {
    conn.execute(
        "UPDATE world_entities SET props_json=?4, rev=?5 WHERE run_id=?1 AND world_id=?2 AND id=?3",
        params![run_id, wid, e.id, props.to_string(), seq],
    )?;
    Ok(())
}

fn add_noticer(conn: &Connection, run_id: &str, wid: &str, e: &Entity, who: &str, seq: i64) -> Result<()> {
    let mut nb = noticed_by(e);
    if !nb.iter().any(|n| n == who) {
        nb.push(who.to_string());
        let mut p = e.props.clone();
        p["noticed_by"] = json!(nb);
        set_props(conn, run_id, wid, e, &p, seq)?;
    }
    Ok(())
}

#[derive(Debug, Default, Serialize)]
pub struct WorldOutcome {
    pub summary: String,
    pub event_ids: Vec<String>,
    pub posted: Vec<Posted>,
}

pub struct WorldCtx<'a> {
    pub room_id: &'a str,
    pub chain_depth: i64,
    pub trigger_event_id: Option<&'a str>,
}

fn world_msg(conn: &Connection, run_id: &str, room: &str, text: &str, audience: Vec<String>, parents: Vec<String>, out: &mut WorldOutcome) -> Result<()> {
    if audience.is_empty() {
        return Ok(());
    }
    let p = comms::post(conn, run_id, PostSpec {
        room_id: room.into(),
        author_id: "world".into(),
        text: text.into(),
        visibility: Some(AudienceScope::Location),
        system_origin: Some(OriginKind::World),
        audience_limit: Some(audience),
        parents,
        chain_depth: 99, // world messages never trigger agent turns by themselves
        ..Default::default()
    })?;
    out.event_ids.push(p.event_id.clone());
    out.posted.push(p);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn obs(conn: &Connection, run_id: &str, room: &str, who: &str, ev: &str, kind: ObsKind, data: Value, seq: i64) -> Result<()> {
    let sim = events::sim_time(conn, run_id)?;
    comms::record_observation(conn, run_id, who, Some(room), ev, ObsChannel::WorldLocal, kind, data, ObsStatus::Observed, "world_visibility_gate_v1", sim, None, seq)?;
    Ok(())
}

/// Apply a validated world action. Every failure path returns `Rejected` before mutating.
pub fn apply(conn: &Connection, run_id: &str, actor: &AgentRow, req: &ActionRequest, ctx: &WorldCtx) -> Result<WorldOutcome> {
    let w = world_for_room(conn, run_id, ctx.room_id)?;
    let wid = &w.world_id;
    let loc = char_location(conn, run_id, wid, &actor.id)?.ok_or_else(|| CoreError::Rejected { code: ReasonCode::NotVisible, message: format!("{} is not in this world", actor.display_name) })?;
    let me = &actor.display_name;
    let mut out = WorldOutcome::default();
    let parents: Vec<String> = ctx.trigger_event_id.map(|t| vec![t.to_string()]).unwrap_or_default();

    match req.action {
        ActionKind::Look => {
            let v = view_for(conn, run_id, wid, &actor.id)?;
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"location": loc}), parents.clone()))?;
            out.event_ids.push(ev.event_id.clone());
            obs(conn, run_id, ctx.room_id, &actor.id, &ev.event_id, ObsKind::LocationView, serde_json::to_value(&v)?, ev.seq)?;
            world_msg(conn, run_id, ctx.room_id, &describe_view(&v), vec![actor.id.clone()], vec![ev.event_id], &mut out)?;
            out.summary = format!("looked around {}", v.name);
        }
        ActionKind::Move => {
            let to_raw = arg(req, "to").or(arg(req, "target")).ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "MOVE requires args.to".into() })?;
            let to = resolve_location(conn, run_id, wid, to_raw)?.ok_or_else(|| CoreError::Rejected { code: ReasonCode::TargetUnknown, message: format!("no place called '{to_raw}'") })?;
            let exits = exits_for(conn, run_id, wid, &loc, &actor.id)?;
            let Some(exit) = exits.iter().find(|e| e.to == to) else {
                return reject(ReasonCode::WorldRuleDenied, format!("there is no known way from here to '{to_raw}'"));
            };
            if exit.locked {
                return reject(ReasonCode::WorldRuleDenied, format!("the way to {} is locked", exit.name));
            }
            let (from_name, _, _) = loc_row(conn, run_id, wid, &loc)?;
            let before = listeners_at(conn, run_id, wid, &loc)?;
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"from": loc, "to": to}), parents.clone()))?;
            conn.execute(
                "UPDATE world_entities SET location_id=?4, rev=?5 WHERE run_id=?1 AND world_id=?2 AND id=?3",
                params![run_id, wid, actor.id, to, ev.seq],
            )?;
            bump(conn, run_id, wid)?;
            out.event_ids.push(ev.event_id.clone());
            let leavers: Vec<String> = before.into_iter().filter(|x| x != &actor.id).collect();
            world_msg(conn, run_id, ctx.room_id, &format!("{me} leaves {from_name} toward {}.", exit.name), leavers.clone(), vec![ev.event_id.clone()], &mut out)?;
            for l in &leavers {
                obs(conn, run_id, ctx.room_id, l, &ev.event_id, ObsKind::ActionSeen, json!({"actor": actor.id, "action": "LEAVE", "toward": to}), ev.seq)?;
            }
            let arrivals: Vec<String> = listeners_at(conn, run_id, wid, &to)?.into_iter().filter(|x| x != &actor.id).collect();
            world_msg(conn, run_id, ctx.room_id, &format!("{me} arrives in {}.", exit.name), arrivals.clone(), vec![ev.event_id.clone()], &mut out)?;
            for a in &arrivals {
                obs(conn, run_id, ctx.room_id, a, &ev.event_id, ObsKind::ActionSeen, json!({"actor": actor.id, "action": "ARRIVE"}), ev.seq)?;
            }
            let v = view_for(conn, run_id, wid, &actor.id)?;
            obs(conn, run_id, ctx.room_id, &actor.id, &ev.event_id, ObsKind::LocationView, serde_json::to_value(&v)?, ev.seq)?;
            world_msg(conn, run_id, ctx.room_id, &describe_view(&v), vec![actor.id.clone()], vec![ev.event_id.clone()], &mut out)?;
            arrival_checks(conn, run_id, &w, actor, &to, &ev, &mut out)?;
            narrate(conn, run_id, &w, ctx.room_id, &ev.event_id, &to, vec![format!("{me} enters {}.", exit.name)], listeners_at(conn, run_id, wid, &to)?, &mut out)?;
            out.summary = format!("moved to {}", exit.name);
        }
        ActionKind::Inspect => {
            let target = arg(req, "target").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "INSPECT requires args.target".into() })?;
            let tl = target.trim().to_lowercase();
            // Inspecting the room or a wall can reveal hidden passages.
            let room_words = ["room", "walls", "wall", "area", "here", "around", "floor"];
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"target": target}), parents.clone()))?;
            out.event_ids.push(ev.event_id.clone());
            if room_words.contains(&tl.as_str()) {
                let found = search_hidden(conn, run_id, &w, actor, &loc, &ev, &mut out)?;
                if !found {
                    world_msg(conn, run_id, ctx.room_id, "You search carefully but find nothing unusual.", vec![actor.id.clone()], vec![ev.event_id.clone()], &mut out)?;
                }
                out.summary = "searched the area".into();
            } else {
                // Hidden-but-present objects can be found by inspecting them by name only after a successful search.
                let e = resolve_target(conn, run_id, wid, &loc, &actor.id, target)?;
                if e.visibility == "NOTICEABLE_IF_INSPECTED" {
                    add_noticer(conn, run_id, wid, &e, &actor.id, ev.seq)?;
                }
                let mut text = format!("{}: {}", e.name, e.props.get("description").and_then(|d| d.as_str()).unwrap_or("Nothing remarkable."));
                if e.kind == "DOOR" {
                    text.push_str(&format!(" It is {}.", e.state.to_lowercase()));
                }
                if let Some(cue) = e.props.get("cue").and_then(|c| c.as_str()) {
                    text.push_str(&format!(" {cue}"));
                    obs(conn, run_id, ctx.room_id, &actor.id, &ev.event_id, ObsKind::VisibleObjectState, json!({"object": e.id, "feature": e.state, "cue": cue}), ev.seq)?;
                }
                // Contents noticeable on inspection (e.g. a map on the shelf).
                for inner in entities_at(conn, run_id, wid, &loc)? {
                    if inner.visibility == "NOTICEABLE_IF_INSPECTED" && inner.id != e.id && inner.props.get("found_with").and_then(|f| f.as_str()) == Some(e.id.as_str()) {
                        add_noticer(conn, run_id, wid, &inner, &actor.id, ev.seq)?;
                        text.push_str(&format!(" You spot {} here.", inner.name));
                    }
                }
                obs(conn, run_id, ctx.room_id, &actor.id, &ev.event_id, ObsKind::ActionResult, json!({"inspected": e.id, "text": text}), ev.seq)?;
                world_msg(conn, run_id, ctx.room_id, &text, vec![actor.id.clone()], vec![ev.event_id.clone()], &mut out)?;
                out.summary = format!("inspected {}", e.name);
            }
        }
        ActionKind::Take => {
            let target = arg(req, "target").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "TAKE requires args.target".into() })?;
            let e = resolve_target(conn, run_id, wid, &loc, &actor.id, target)?;
            stale_check(&e, req)?;
            if holder_of(conn, run_id, wid, &e.id)?.is_some() {
                return reject(ReasonCode::WorldRuleDenied, format!("{} is already carried", e.name));
            }
            if !e.props.get("takeable").and_then(|t| t.as_bool()).unwrap_or(false) {
                return reject(ReasonCode::WorldRuleDenied, format!("{} cannot be taken", e.name));
            }
            if inventory(conn, run_id, wid, &actor.id)?.len() as i64 >= w.capacity {
                return reject(ReasonCode::WorldRuleDenied, format!("inventory full (capacity {})", w.capacity));
            }
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"target": e.id, "from_location": loc}), parents.clone()))?;
            conn.execute("UPDATE world_entities SET location_id=NULL, rev=?4 WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, wid, e.id, ev.seq])?;
            conn.execute("INSERT INTO inventories(run_id, world_id, holder_id, entity_id, acquired_seq) VALUES (?1,?2,?3,?4,?5)", params![run_id, wid, actor.id, e.id, ev.seq])?;
            if let Some(tr) = e.props.get("trace_on_take").filter(|t| !t.is_null()) {
                let tr: TraceSpec = serde_json::from_value(tr.clone())?;
                if let Some(f) = entity(conn, run_id, wid, &tr.feature_id)? {
                    let mut p = f.props.clone();
                    p["cue"] = json!(tr.cue);
                    conn.execute(
                        "UPDATE world_entities SET state=?4, props_json=?5, rev=?6 WHERE run_id=?1 AND world_id=?2 AND id=?3",
                        params![run_id, wid, f.id, tr.state, p.to_string(), ev.seq],
                    )?;
                }
            }
            bump(conn, run_id, wid)?;
            out.event_ids.push(ev.event_id.clone());
            let here = listeners_at(conn, run_id, wid, &loc)?;
            world_msg(conn, run_id, ctx.room_id, &format!("{me} takes the {}.", e.name), here.clone(), vec![ev.event_id.clone()], &mut out)?;
            for h in here.iter().filter(|h| *h != &actor.id) {
                obs(conn, run_id, ctx.room_id, h, &ev.event_id, ObsKind::ActionSeen, json!({"actor": actor.id, "action": "TAKE", "object": e.id}), ev.seq)?;
            }
            out.summary = format!("took {}", e.name);
        }
        ActionKind::Drop => {
            let target = arg(req, "target").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "DROP requires args.target".into() })?;
            let inv = inventory(conn, run_id, wid, &actor.id)?;
            let tl = target.to_lowercase();
            let Some(e) = inv.into_iter().find(|e| e.id.to_lowercase() == tl || e.name.to_lowercase() == tl) else {
                return reject(ReasonCode::WorldRuleDenied, format!("you are not carrying '{target}'"));
            };
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"target": e.id, "location": loc}), parents.clone()))?;
            conn.execute("DELETE FROM inventories WHERE run_id=?1 AND world_id=?2 AND entity_id=?3", params![run_id, wid, e.id])?;
            conn.execute("UPDATE world_entities SET location_id=?4, visibility='VISIBLE', rev=?5 WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, wid, e.id, loc, ev.seq])?;
            bump(conn, run_id, wid)?;
            out.event_ids.push(ev.event_id.clone());
            world_msg(conn, run_id, ctx.room_id, &format!("{me} sets down the {}.", e.name), listeners_at(conn, run_id, wid, &loc)?, vec![ev.event_id.clone()], &mut out)?;
            out.summary = format!("dropped {}", e.name);
        }
        ActionKind::Use => {
            let item = arg(req, "item").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "USE requires args.item".into() })?;
            let target = arg(req, "target").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "USE requires args.target".into() })?;
            let il = item.to_lowercase();
            let Some(it) = inventory(conn, run_id, wid, &actor.id)?.into_iter().find(|e| e.id.to_lowercase() == il || e.name.to_lowercase() == il) else {
                return reject(ReasonCode::WorldRuleDenied, format!("you are not carrying '{item}'"));
            };
            let t = resolve_target(conn, run_id, wid, &loc, &actor.id, target)?;
            stale_check(&t, req)?;
            let unlocks = it.props.get("unlocks").and_then(|u| u.as_str());
            if unlocks != Some(t.id.as_str()) {
                return reject(ReasonCode::WorldRuleDenied, format!("the {} does nothing to the {}", it.name, t.name));
            }
            let new_state = if t.state == "LOCKED" { "UNLOCKED" } else { "LOCKED" };
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"item": it.id, "target": t.id, "state": new_state}), parents.clone()))?;
            conn.execute("UPDATE world_entities SET state=?4, rev=?5 WHERE run_id=?1 AND world_id=?2 AND id=?3", params![run_id, wid, t.id, new_state, ev.seq])?;
            bump(conn, run_id, wid)?;
            out.event_ids.push(ev.event_id.clone());
            let here = listeners_at(conn, run_id, wid, &loc)?;
            world_msg(conn, run_id, ctx.room_id, &format!("{me} uses the {} on the {}. It is now {}.", it.name, t.name, new_state.to_lowercase()), here, vec![ev.event_id.clone()], &mut out)?;
            out.summary = format!("{} the {}", new_state.to_lowercase(), t.name);
        }
        ActionKind::Speak | ActionKind::Ask | ActionKind::Whisper => {
            let text = req.message_text.clone().or_else(|| arg(req, "text").map(String::from)).unwrap_or_default();
            if text.trim().is_empty() {
                return reject(ReasonCode::InvalidSchema, "speech requires message_text");
            }
            let here = listeners_at(conn, run_id, wid, &loc)?;
            let (vis, audience, recips) = if req.action == ActionKind::Whisper {
                let to = arg(req, "to").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "WHISPER requires args.to".into() })?;
                let tid = resolve_present(conn, run_id, wid, &here, to)?;
                (AudienceScope::Location, vec![actor.id.clone(), tid.clone()], vec![tid])
            } else {
                (AudienceScope::Location, here.clone(), vec![])
            };
            let p = comms::post(conn, run_id, PostSpec {
                room_id: ctx.room_id.into(),
                author_id: actor.id.clone(),
                text,
                visibility: Some(vis),
                recipients: recips,
                audience_limit: Some(audience),
                reaction_lane: req.reaction_lane.clone(),
                confidence: req.confidence,
                chain_depth: ctx.chain_depth,
                parents: parents.clone(),
                is_demo: actor.cognition == Cognition::L0Scripted && actor.kind != AgentKind::Human,
                ..Default::default()
            })?;
            out.event_ids.push(p.event_id.clone());
            out.posted.push(p);
            out.summary = "spoke".into();
        }
        ActionKind::Summon => {
            out = summon(conn, run_id, &w, actor, &loc, req, ctx)?;
        }
        ActionKind::Wait | ActionKind::NoOp => {
            let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({}), parents))?;
            out.event_ids.push(ev.event_id);
            out.summary = "waited".into();
        }
        _ => return reject(ReasonCode::CapabilityDenied, format!("{} is not a world action", req.action.as_str())),
    }
    Ok(out)
}

fn stale_check(e: &Entity, req: &ActionRequest) -> Result<()> {
    if req.based_on_event_seq > 0 && e.rev > req.based_on_event_seq {
        return reject(ReasonCode::StaleState, format!("{} changed after your last observation (rev {} > seq {})", e.name, e.rev, req.based_on_event_seq));
    }
    Ok(())
}

fn resolve_present(conn: &Connection, run_id: &str, wid: &str, here: &[String], to: &str) -> Result<String> {
    let tl = to.trim().trim_start_matches('@').to_lowercase();
    for h in here {
        let name = crate::agent::display_name(conn, run_id, h).to_lowercase();
        if h.to_lowercase() == tl || name == tl {
            return Ok(h.clone());
        }
    }
    let _ = wid;
    reject(ReasonCode::NotVisible, format!("'{to}' is not here to hear you"))
}

fn act_event(actor: &AgentRow, req: &ActionRequest, room: &str, detail: Value, parents: Vec<String>) -> NewEvent {
    let kind = if actor.kind == AgentKind::Human { OriginKind::Human } else { OriginKind::Agent };
    NewEvent::new(EventType::ActionExecuted, kind, &actor.id, AudienceScope::OperatorOnly, json!({
        "request_id": req.request_id, "action": req.action, "args": req.args, "detail": detail
    }))
    .room(room)
    .parents(parents)
}

/// On arrival: observation checks for traces (SPDE-style explicit rule) and ambient sounds.
fn arrival_checks(conn: &Connection, run_id: &str, w: &WorldRef, actor: &AgentRow, loc: &str, ev: &EventEnvelope, out: &mut WorldOutcome) -> Result<()> {
    let wid = &w.world_id;
    let perception = entity(conn, run_id, wid, &actor.id)?.and_then(|e| e.props.get("perception").and_then(|p| p.as_i64())).unwrap_or(0);
    for f in entities_at(conn, run_id, wid, loc)? {
        if f.kind == "SOUND" {
            let d = f.props.get("description").and_then(|d| d.as_str()).unwrap_or("a sound");
            obs(conn, run_id, &w.room_id, &actor.id, &ev.event_id, ObsKind::Sound, json!({"sound": d, "source": "unknown"}), ev.seq)?;
            world_msg(conn, run_id, &w.room_id, &format!("You hear {d}. You cannot see its source."), vec![actor.id.clone()], vec![ev.event_id.clone()], out)?;
            continue;
        }
        let (Some(dc), Some(cue)) = (f.props.get("notice_dc").and_then(|d| d.as_i64()), f.props.get("cue").and_then(|c| c.as_str())) else { continue };
        let roll = d20(w.seed, &format!("notice:{}:{}", actor.id, f.id), ev.seq);
        let success = roll + perception >= dc;
        // Operator-only record of the check, including the roll.
        let chk = events::append(conn, run_id, NewEvent::new(EventType::WorldStateChanged, OriginKind::World, "world", AudienceScope::OperatorOnly, json!({
            "check": "notice", "observer": actor.id, "feature": f.id, "roll": roll, "bonus": perception, "dc": dc, "success": success
        })).room(&w.room_id).parents(vec![ev.event_id.clone()]))?;
        out.event_ids.push(chk.event_id.clone());
        if success {
            obs(conn, run_id, &w.room_id, &actor.id, &chk.event_id, ObsKind::VisibleObjectState, json!({"object": f.id, "feature": f.state, "cue": cue}), chk.seq)?;
            world_msg(conn, run_id, &w.room_id, &format!("You notice: {cue}"), vec![actor.id.clone()], vec![chk.event_id.clone()], out)?;
        }
    }
    Ok(())
}

fn search_hidden(conn: &Connection, run_id: &str, w: &WorldRef, actor: &AgentRow, loc: &str, ev: &EventEnvelope, out: &mut WorldOutcome) -> Result<bool> {
    let wid = &w.world_id;
    let mut st = conn.prepare("SELECT to_loc, discover_dc, discovered_by_json, door_entity_id FROM world_edges WHERE run_id=?1 AND world_id=?2 AND from_loc=?3 AND hidden=1")?;
    let rows = st
        .query_map(params![run_id, wid, loc], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?, r.get::<_, String>(2)?, r.get::<_, Option<String>>(3)?)))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let perception = entity(conn, run_id, wid, &actor.id)?.and_then(|e| e.props.get("perception").and_then(|p| p.as_i64())).unwrap_or(0);
    let mut found = false;
    for (to, dc, disc, door) in rows {
        let mut d: Vec<String> = serde_json::from_str(&disc).unwrap_or_default();
        if d.contains(&actor.id) {
            continue;
        }
        let roll = d20(w.seed, &format!("search:{}:{}", actor.id, to), ev.seq);
        let success = roll + perception >= dc;
        let chk = events::append(conn, run_id, NewEvent::new(EventType::WorldStateChanged, OriginKind::World, "world", AudienceScope::OperatorOnly, json!({
            "check": "search", "searcher": actor.id, "edge": [loc, to], "roll": roll, "bonus": perception, "dc": dc, "success": success
        })).room(&w.room_id).parents(vec![ev.event_id.clone()]))?;
        out.event_ids.push(chk.event_id.clone());
        if success {
            d.push(actor.id.clone());
            // Discovery is per observer and applies to both directions.
            conn.execute(
                "UPDATE world_edges SET discovered_by_json=?5 WHERE run_id=?1 AND world_id=?2 AND ((from_loc=?3 AND to_loc=?4) OR (from_loc=?4 AND to_loc=?3))",
                params![run_id, wid, loc, to, serde_json::to_string(&d)?],
            )?;
            if let Some(door) = door {
                if let Some(de) = entity(conn, run_id, wid, &door)? {
                    add_noticer(conn, run_id, wid, &de, &actor.id, chk.seq)?;
                }
            }
            let (to_name, _, _) = loc_row(conn, run_id, wid, &to)?;
            obs(conn, run_id, &w.room_id, &actor.id, &chk.event_id, ObsKind::VisibleObjectState, json!({"discovered_passage_to": to}), chk.seq)?;
            world_msg(conn, run_id, &w.room_id, &format!("Behind loose stones you find a narrow passage leading toward the {to_name}."), vec![actor.id.clone()], vec![chk.event_id.clone()], out)?;
            found = true;
        }
    }
    Ok(found)
}

fn summon(conn: &Connection, run_id: &str, w: &WorldRef, actor: &AgentRow, loc: &str, req: &ActionRequest, ctx: &WorldCtx) -> Result<WorldOutcome> {
    let wid = &w.world_id;
    let me = entity(conn, run_id, wid, &actor.id)?.ok_or_else(|| CoreError::Rejected { code: ReasonCode::NotVisible, message: "not in world".into() })?;
    let abilities: Vec<String> = me.props.get("abilities").and_then(|a| serde_json::from_value(a.clone()).ok()).unwrap_or_default();
    if !abilities.iter().any(|a| a == "summon_familiar") {
        return reject(ReasonCode::CapabilityDenied, format!("{} has no summoning ability", actor.display_name));
    }
    let slots = me.props.get("spell_slots").and_then(|s| s.as_i64()).unwrap_or(0);
    if slots < 1 {
        return reject(ReasonCode::WorldRuleDenied, "no spell slots remaining");
    }
    if crate::auth::is_child(actor) {
        return reject(ReasonCode::SpawnDepthExceeded, "summoned entities cannot summon");
    }
    let name = arg(req, "name").unwrap_or("Ash").chars().filter(|c| c.is_alphanumeric() || *c == ' ').take(24).collect::<String>();
    let name = if name.trim().is_empty() { "Ash".to_string() } else { name.trim().to_string() };
    let target = match arg(req, "target") {
        Some(t) => resolve_location(conn, run_id, wid, t)?.ok_or_else(|| CoreError::Rejected { code: ReasonCode::TargetUnknown, message: format!("unknown place '{t}'") })?,
        None => loc.to_string(),
    };
    let active: i64 = conn.query_row(
        "SELECT COUNT(*) FROM spawn_instances WHERE run_id=?1 AND parent_agent_id=?2 AND state='ACTIVE' AND request_id LIKE 'summon_%'",
        params![run_id, actor.id],
        |r| r.get(0),
    )?;
    if active > 0 {
        return reject(ReasonCode::SpawnCapReached, "a familiar is already active");
    }
    let ev = events::append(conn, run_id, act_event(actor, req, ctx.room_id, json!({"ability": "summon_familiar", "name": name, "target": target, "slot_consumed": true}), vec![]))?;
    let mut p = me.props.clone();
    p["spell_slots"] = json!(slots - 1);
    set_props(conn, run_id, wid, &me, &p, ev.seq)?;
    let child_id = format!("familiar_{}", crate::ids::slug(&name));
    let child_id = if crate::auth::load_agent(conn, run_id, &child_id)?.is_some() { format!("{child_id}_{}", ev.seq) } else { child_id };
    let mut prof = crate::agent::profile_from_seed(&crate::agent::ProfileSeed {
        id: &child_id,
        name: &name,
        icon: "🐦‍⬛",
        role: "familiar",
        persona: &format!("A raven familiar summoned by {} to scout {}. Reports only what it perceives.", actor.display_name, target),
        directive: &format!("Go to {target}, listen and look, then report to your summoner. Do not guess sources you did not see."),
        cognition: Cognition::L0Scripted,
        route: "demo",
        soma: SomaMode::Light,
        demo_policy: Some("familiar"),
        actions: vec![ActionKind::Move, ActionKind::Look, ActionKind::ReportFinal, ActionKind::ReportProgress, ActionKind::Wait, ActionKind::NoOp],
        motivation: "serve the summoner",
        style: "terse",
    });
    prof.limits.turn_output_tokens = 80;
    crate::agent::create_agent(conn, run_id, &prof, AgentKind::WorldEntity, Some(&actor.id), Some(SpawnType::WorldEntity))?;
    crate::room::add_member(conn, run_id, ctx.room_id, &child_id, "familiar", "PARENT_ONLY")?;
    conn.execute(
        "INSERT INTO world_entities(run_id, world_id, id, name, kind, location_id, state, visibility, props_json, rev) VALUES (?1,?2,?3,?4,'FAMILIAR',?5,'active','VISIBLE',?6,?7)",
        params![run_id, wid, child_id, name, loc, json!({"target": target, "summoner": actor.id, "perception": 3}).to_string(), ev.seq],
    )?;
    let now = crate::ids::now_ms();
    let req_id = format!("summon_{child_id}");
    conn.execute(
        "INSERT INTO spawn_requests(run_id, request_id, parent_agent_id, room_id, spawn_type, payload_json, state, reason_code, effective_json, child_agent_id, created_seq, updated_seq)
         VALUES (?1,?2,?3,?4,'WORLD_ENTITY',?5,'ACTIVE','OK',?6,?7,?8,?8)",
        params![run_id, req_id, actor.id, ctx.room_id, json!({"summon": name, "target": target}).to_string(), json!({"max_turns": 4, "ttl_seconds": 300}).to_string(), child_id, ev.seq],
    )?;
    conn.execute(
        "INSERT INTO spawn_instances(run_id, child_agent_id, request_id, parent_agent_id, room_id, state, max_turns, max_tokens, ttl_seconds, started_wall_ms, expires_wall_ms)
         VALUES (?1,?2,?3,?4,?5,'ACTIVE',4,400,300,?6,?7)",
        params![run_id, child_id, req_id, actor.id, ctx.room_id, now, now + 300_000],
    )?;
    bump(conn, run_id, wid)?;
    let mut out = WorldOutcome::default();
    out.event_ids.push(ev.event_id.clone());
    let here = listeners_at(conn, run_id, wid, loc)?;
    world_msg(conn, run_id, ctx.room_id, &format!("Validated: {} spends a spell slot. A raven familiar, {name}, takes shape.", actor.display_name), here, vec![ev.event_id.clone()], &mut out)?;
    out.summary = format!("summoned {name}");
    Ok(out)
}

/// Dismiss a familiar (expiry, completion or operator stop): remove from world.
pub fn dismiss_entity(conn: &Connection, run_id: &str, room_id: &str, entity_id: &str) -> Result<()> {
    let Ok(w) = world_for_room(conn, run_id, room_id) else { return Ok(()) };
    let seq = events::current_seq(conn, run_id)?;
    if let Some(e) = entity(conn, run_id, &w.world_id, entity_id)? {
        if let Some(loc) = &e.location {
            let here = listeners_at(conn, run_id, &w.world_id, loc)?.into_iter().filter(|h| h != entity_id).collect::<Vec<_>>();
            let mut out = WorldOutcome::default();
            world_msg(conn, run_id, room_id, &format!("{} dissolves into a scatter of feathers.", e.name), here, vec![], &mut out)?;
        }
        conn.execute(
            "UPDATE world_entities SET location_id=NULL, state='dismissed', rev=?4 WHERE run_id=?1 AND world_id=?2 AND id=?3",
            params![run_id, w.world_id, entity_id, seq],
        )?;
    }
    Ok(())
}

/// Shortest path over edges known to `viewer` (used by scripted familiars/NPCs).
pub fn next_step_toward(conn: &Connection, run_id: &str, wid: &str, from: &str, to: &str, viewer: &str) -> Result<Option<String>> {
    if from == to {
        return Ok(None);
    }
    let mut prev: std::collections::HashMap<String, String> = Default::default();
    let mut q = VecDeque::from([from.to_string()]);
    let mut seen = HashSet::from([from.to_string()]);
    while let Some(cur) = q.pop_front() {
        for e in exits_for(conn, run_id, wid, &cur, viewer)? {
            if e.locked || !seen.insert(e.to.clone()) {
                continue;
            }
            prev.insert(e.to.clone(), cur.clone());
            if e.to == to {
                let mut step = to.to_string();
                while prev.get(&step).map(|p| p != from).unwrap_or(false) {
                    step = prev[&step].clone();
                }
                return Ok(Some(step));
            }
            q.push_back(e.to.clone());
        }
    }
    Ok(None)
}

fn describe_view(v: &LocationView) -> String {
    let mut s = format!("{} — {}.", v.name, v.ambient);
    if !v.description.is_empty() {
        s.push(' ');
        s.push_str(&v.description);
    }
    if !v.objects.is_empty() {
        s.push_str(&format!(" You see: {}.", v.objects.iter().map(|o| if o.state.is_empty() { o.name.clone() } else { format!("{} ({})", o.name, o.state) }).collect::<Vec<_>>().join(", ")));
    }
    if !v.characters.is_empty() {
        s.push_str(&format!(" Present: {}.", v.characters.join(", ")));
    }
    let exits: Vec<String> = v.exits.iter().map(|e| format!("{}{}", e.name, if e.locked { " (locked)" } else { "" })).collect();
    s.push_str(&format!(" Exits: {}.", if exits.is_empty() { "none".into() } else { exits.join(", ") }));
    s
}

#[allow(clippy::too_many_arguments)]
fn narrate(conn: &Connection, run_id: &str, w: &WorldRef, room: &str, ev_id: &str, loc: &str, facts: Vec<String>, audience: Vec<String>, out: &mut WorldOutcome) -> Result<()> {
    if w.resolution == NarrativeResolution::Minimal || audience.is_empty() {
        return Ok(());
    }
    let narrator: Option<String> = conn
        .query_row(
            "SELECT m.agent_id FROM memberships m JOIN agents a ON a.run_id=m.run_id AND a.id=m.agent_id WHERE m.run_id=?1 AND m.room_id=?2 AND a.kind='NARRATOR' AND a.status='ACTIVE' LIMIT 1",
            params![run_id, room],
            |r| r.get(0),
        )
        .optional()?;
    let Some(narrator) = narrator else { return Ok(()) };
    let (name, ambient, _) = loc_row(conn, run_id, &w.world_id, loc)?;
    let packet = NarrationPacket {
        source_event_id: ev_id.to_string(),
        location_name: name,
        ambient,
        facts,
        allowed_names: vec![],
        genre: w.genre.clone(),
        resolution: w.resolution,
    };
    let seq = events::current_seq(conn, run_id)?;
    let text = narrative::render(&packet, w.seed, seq);
    let p = comms::post(conn, run_id, PostSpec {
        room_id: room.into(),
        author_id: narrator,
        text,
        visibility: Some(AudienceScope::Location),
        audience_limit: Some(audience),
        is_demo: true,
        chain_depth: 99,
        parents: vec![ev_id.to_string()],
        ..Default::default()
    })?;
    out.event_ids.push(p.event_id.clone());
    out.posted.push(p);
    Ok(())
}

/// Operator truth: full world state (never sent to agents).
#[derive(Serialize)]
pub struct WorldTruth {
    pub world: WorldRef,
    pub entities: Vec<Entity>,
    pub inventories: Vec<(String, String)>,
    pub edges: Vec<Value>,
}

pub fn truth(conn: &Connection, run_id: &str, room_id: &str) -> Result<WorldTruth> {
    let w = world_for_room(conn, run_id, room_id)?;
    let entities = all_entities(conn, run_id, &w.world_id)?;
    let mut st = conn.prepare("SELECT holder_id, entity_id FROM inventories WHERE run_id=?1 AND world_id=?2")?;
    let inventories = st.query_map(params![run_id, w.world_id], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    let mut st = conn.prepare("SELECT from_loc, to_loc, hidden, discovered_by_json, door_entity_id FROM world_edges WHERE run_id=?1 AND world_id=?2")?;
    let edges = st
        .query_map(params![run_id, w.world_id], |r| {
            Ok(json!({"from": r.get::<_, String>(0)?, "to": r.get::<_, String>(1)?, "hidden": r.get::<_, i64>(2)? != 0, "discovered_by": r.get::<_, String>(3)?, "door": r.get::<_, Option<String>>(4)?}))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(WorldTruth { world: w, entities, inventories, edges })
}

pub fn locations(conn: &Connection, run_id: &str, wid: &str) -> Result<Vec<(String, String)>> {
    let mut st = conn.prepare("SELECT id, name FROM locations WHERE run_id=?1 AND world_id=?2 ORDER BY id")?;
    let v = st.query_map(params![run_id, wid], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(v)
}

/// Presentation-only change; journaled by the caller as an intervention.
pub fn set_resolution(conn: &Connection, run_id: &str, room_id: &str, res: NarrativeResolution) -> Result<()> {
    if let Ok(w) = world_for_room(conn, run_id, room_id) {
        conn.execute("UPDATE worlds SET narrative_resolution=?3 WHERE run_id=?1 AND world_id=?2", params![run_id, w.world_id, res.as_str()])?;
    }
    Ok(())
}
