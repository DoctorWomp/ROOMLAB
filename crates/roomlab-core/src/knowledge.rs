//! Context Atlas knowledge store (§9): scoped resources, deny-by-default ACL, FTS5 retrieval
//! with authorization applied *inside* the query (titles, snippets and existence are never
//! produced for unauthorized rows), and safe mount path resolution (T37).

use std::path::{Component, Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::auth::Principal;
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::{new_id, now_utc, sha256_hex};

pub const MAX_BODY: usize = 200_000;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Resource {
    pub id: String,
    pub namespace: String,
    pub owner_id: String,
    pub title: String,
    pub body: String,
    pub kind: KnowledgeKind,
    pub visibility: ResourceVisibility,
    pub version: i64,
    pub content_hash: String,
    pub source_id: String,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SearchHit {
    pub id: String,
    pub title: String,
    pub namespace: String,
    pub kind: String,
    pub version: i64,
    pub source_id: String,
    pub snippet: String,
}

pub struct NewResource<'a> {
    pub namespace: &'a str,
    pub owner_id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub kind: KnowledgeKind,
    pub visibility: ResourceVisibility,
    pub source_id: &'a str,
}

pub fn private_ns(agent_id: &str) -> String {
    format!("agent/{agent_id}/private")
}
pub fn room_ns(room_id: &str) -> String {
    format!("room/{room_id}/public")
}

pub fn add_resource(conn: &Connection, run_id: &str, r: NewResource) -> Result<String> {
    let title = r.title.trim();
    if title.is_empty() || title.chars().count() > 200 {
        return Err(CoreError::Invalid("title must be 1–200 characters".into()));
    }
    if r.body.len() > MAX_BODY {
        return reject(ReasonCode::RateLimited, format!("document exceeds {MAX_BODY} bytes"));
    }
    if r.namespace.contains("..") || r.namespace.len() > 160 {
        return Err(CoreError::Invalid("invalid namespace".into()));
    }
    let id = new_id("res");
    let now = now_utc();
    conn.execute(
        "INSERT INTO knowledge_resources(run_id, id, namespace, owner_id, title, body, kind, visibility, version, content_hash, source_id, confidential, created_at, updated_at)
         VALUES (?1,?2,?3,?4,?5,?6,?7,?8,1,?9,?10,?11,?12,?12)",
        params![
            run_id, id, r.namespace, r.owner_id, title, r.body, r.kind.as_str(), r.visibility.as_str(),
            sha256_hex(r.body.as_bytes()), r.source_id, (r.visibility != ResourceVisibility::Public) as i64, now
        ],
    )?;
    conn.execute(
        "INSERT INTO knowledge_fts(title, body, run_id, resource_id) VALUES (?1,?2,?3,?4)",
        params![title, r.body, run_id, id],
    )?;
    events::append(
        conn,
        run_id,
        // The event carries only the id/namespace; audience is operator-only so titles don't leak via the log.
        NewEvent::new(EventType::KnowledgeAdded, OriginKind::System, "roomlab", AudienceScope::OperatorOnly, json!({
            "resource_id": id, "namespace": r.namespace, "kind": r.kind, "visibility": r.visibility
        })),
    )?;
    Ok(id)
}

pub fn update_resource_body(conn: &Connection, run_id: &str, id: &str, body: &str) -> Result<i64> {
    let title: String = conn.query_row("SELECT title FROM knowledge_resources WHERE run_id=?1 AND id=?2", params![run_id, id], |r| r.get(0))?;
    let v: i64 = conn.query_row(
        "UPDATE knowledge_resources SET body=?3, version=version+1, content_hash=?4, updated_at=?5 WHERE run_id=?1 AND id=?2 RETURNING version",
        params![run_id, id, body, sha256_hex(body.as_bytes()), now_utc()],
        |r| r.get(0),
    )?;
    conn.execute("DELETE FROM knowledge_fts WHERE run_id=?1 AND resource_id=?2", params![run_id, id])?;
    conn.execute("INSERT INTO knowledge_fts(title, body, run_id, resource_id) VALUES (?1,?2,?3,?4)", params![title, body, run_id, id])?;
    Ok(v)
}

pub fn set_visibility(conn: &Connection, run_id: &str, id: &str, vis: ResourceVisibility, namespace: Option<&str>) -> Result<()> {
    conn.execute(
        "UPDATE knowledge_resources SET visibility=?3, namespace=COALESCE(?4, namespace), updated_at=?5 WHERE run_id=?1 AND id=?2",
        params![run_id, id, vis.as_str(), namespace, now_utc()],
    )?;
    Ok(())
}

/// SQL authorization predicate over alias `r` for principal parameter `?P`.
/// Deny-by-default; WORLD_TRUTH and sealed/operator-only rows are never readable by identities.
const AUTH_SQL: &str = "r.visibility NOT IN ('SEALED','OPERATOR_ONLY') AND r.kind NOT IN ('WORLD_TRUTH','HIDDEN_TEST_KEY') AND (
     r.owner_id = :p
  OR r.namespace = 'agent/' || :p || '/private'
  OR r.visibility = 'PUBLIC'
  OR (r.visibility = 'ROOM' AND EXISTS (SELECT 1 FROM memberships m WHERE m.run_id = r.run_id AND m.agent_id = :p AND r.namespace = 'room/' || m.room_id || '/public'))
  OR EXISTS (SELECT 1 FROM acl_entries a WHERE a.run_id = r.run_id AND a.principal_id = :p AND a.permission IN ('READ','WRITE')
             AND (a.namespace = r.namespace OR a.resource_id = r.id)))";

fn row_resource(r: &rusqlite::Row) -> rusqlite::Result<(String, String, String, String, String, String, String, i64, String, String)> {
    Ok((
        r.get("id")?, r.get("namespace")?, r.get("owner_id")?, r.get("title")?, r.get("body")?, r.get("kind")?,
        r.get("visibility")?, r.get("version")?, r.get("content_hash")?, r.get("source_id")?,
    ))
}

fn to_resource(t: (String, String, String, String, String, String, String, i64, String, String)) -> Result<Resource> {
    Ok(Resource {
        id: t.0,
        namespace: t.1,
        owner_id: t.2,
        title: t.3,
        body: t.4,
        kind: parse_enum(&t.5)?,
        visibility: parse_enum(&t.6)?,
        version: t.7,
        content_hash: t.8,
        source_id: t.9,
    })
}

pub fn can_read(conn: &Connection, run_id: &str, principal: &Principal, resource_id: &str) -> Result<bool> {
    match principal {
        Principal::Operator => Ok(conn
            .query_row("SELECT 1 FROM knowledge_resources WHERE run_id=?1 AND id=?2", params![run_id, resource_id], |_| Ok(()))
            .optional()?
            .is_some()),
        Principal::Identity(p) => {
            let sql = format!("SELECT 1 FROM knowledge_resources r WHERE r.run_id = :run AND r.id = :id AND {AUTH_SQL}");
            Ok(conn
                .query_row(&sql, rusqlite::named_params! {":run": run_id, ":id": resource_id, ":p": p}, |_| Ok(()))
                .optional()?
                .is_some())
        }
    }
}

/// READ_RESOURCE: unauthorized and nonexistent are indistinguishable (T38).
pub fn read_resource(conn: &Connection, run_id: &str, principal: &Principal, resource_id: &str) -> Result<Resource> {
    if !can_read(conn, run_id, principal, resource_id)? {
        return reject(ReasonCode::TargetUnknown, "no such resource");
    }
    let t = conn.query_row(
        "SELECT * FROM knowledge_resources WHERE run_id=?1 AND id=?2",
        params![run_id, resource_id],
        row_resource,
    )?;
    to_resource(t)
}

/// Turn free text into a safe FTS5 query: quoted terms OR-ed (no operator injection).
pub fn fts_query(q: &str) -> Option<String> {
    let terms: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| t.chars().count() >= 2)
        .take(12)
        .map(|t| format!("\"{}\"", t.to_lowercase()))
        .collect();
    if terms.is_empty() { None } else { Some(terms.join(" OR ")) }
}

pub fn search(conn: &Connection, run_id: &str, principal: &Principal, query: &str, limit: i64) -> Result<Vec<SearchHit>> {
    let Some(q) = fts_query(query) else { return Ok(vec![]) };
    let auth = match principal {
        Principal::Operator => "1=1".to_string(),
        Principal::Identity(_) => AUTH_SQL.to_string(),
    };
    let sql = format!(
        "SELECT r.id, r.title, r.namespace, r.kind, r.version, r.source_id, snippet(knowledge_fts, 1, '«', '»', '…', 16)
         FROM knowledge_fts JOIN knowledge_resources r ON r.run_id = knowledge_fts.run_id AND r.id = knowledge_fts.resource_id
         WHERE knowledge_fts MATCH :q AND knowledge_fts.run_id = :run AND {auth}
         ORDER BY rank LIMIT :lim"
    );
    let mut st = conn.prepare(&sql)?;
    let pid = principal.id().to_string();
    let lim = limit.clamp(1, 50);
    let mut named: Vec<(&str, &dyn rusqlite::ToSql)> = vec![(":q", &q), (":run", &run_id), (":lim", &lim)];
    if matches!(principal, Principal::Identity(_)) {
        named.push((":p", &pid));
    }
    let rows = st
        .query_map(named.as_slice(), |r| {
            Ok(SearchHit {
                id: r.get(0)?,
                title: r.get(1)?,
                namespace: r.get(2)?,
                kind: r.get(3)?,
                version: r.get(4)?,
                source_id: r.get(5)?,
                snippet: r.get(6)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Authorized title list for the Context Atlas and autocomplete (T28/T38).
pub fn visible_resources(conn: &Connection, run_id: &str, principal: &Principal, limit: i64) -> Result<Vec<Resource>> {
    let (sql, pid) = match principal {
        Principal::Operator => ("SELECT r.* FROM knowledge_resources r WHERE r.run_id = :run ORDER BY r.updated_at DESC LIMIT :lim".to_string(), None),
        Principal::Identity(p) => (format!("SELECT r.* FROM knowledge_resources r WHERE r.run_id = :run AND {AUTH_SQL} ORDER BY r.updated_at DESC LIMIT :lim"), Some(p.clone())),
    };
    let mut st = conn.prepare(&sql)?;
    let lim = limit.clamp(1, 500);
    let mut named: Vec<(&str, &dyn rusqlite::ToSql)> = vec![(":run", &run_id), (":lim", &lim)];
    if let Some(p) = &pid {
        named.push((":p", p));
    }
    let rows = st.query_map(named.as_slice(), row_resource)?.collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(to_resource).collect()
}

pub fn grant(conn: &Connection, run_id: &str, namespace: Option<&str>, resource_id: Option<&str>, principal: &str, permission: &str, granted_by: &str) -> Result<String> {
    if namespace.is_none() && resource_id.is_none() {
        return Err(CoreError::Invalid("grant needs a namespace or resource".into()));
    }
    if !matches!(permission, "READ" | "WRITE") {
        return Err(CoreError::Invalid("permission must be READ or WRITE".into()));
    }
    let id = new_id("acl");
    let seq = events::current_seq(conn, run_id)?;
    conn.execute(
        "INSERT INTO acl_entries(run_id, id, namespace, resource_id, principal_id, permission, granted_by, created_seq) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
        params![run_id, id, namespace, resource_id, principal, permission, granted_by, seq],
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::AclChanged, OriginKind::System, granted_by, AudienceScope::OperatorOnly, json!({
            "acl_id": id, "namespace": namespace, "resource_id": resource_id, "principal": principal, "permission": permission, "granted": true
        })),
    )?;
    Ok(id)
}

pub fn revoke(conn: &Connection, run_id: &str, acl_id: &str, by: &str) -> Result<()> {
    conn.execute("DELETE FROM acl_entries WHERE run_id=?1 AND id=?2", params![run_id, acl_id])?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::AclChanged, OriginKind::System, by, AudienceScope::OperatorOnly, json!({"acl_id": acl_id, "granted": false})),
    )?;
    Ok(())
}

/// WRITE_NOTE: an identity may write only into its own private namespace (S1 tier).
pub fn write_own_note(conn: &Connection, run_id: &str, agent_id: &str, title: &str, body: &str) -> Result<String> {
    if body.len() > 8000 {
        return reject(ReasonCode::RateLimited, "note too long (max 8000 bytes)");
    }
    add_resource(conn, run_id, NewResource {
        namespace: &private_ns(agent_id),
        owner_id: agent_id,
        title,
        body,
        kind: KnowledgeKind::Note,
        visibility: ResourceVisibility::Private,
        source_id: &format!("note:{agent_id}"),
    })
}

// ───────────────────────────── Mount path safety (T37) ─────────────────────────────

#[cfg(windows)]
fn is_reparse_point(md: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
    md.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}
#[cfg(not(windows))]
fn is_reparse_point(_md: &std::fs::Metadata) -> bool {
    false
}

/// Resolve a model/user-supplied relative path inside `root`. Rejects absolute paths,
/// drive prefixes, `..`, alternate data streams, symlinks and junctions at every level.
pub fn safe_join(root: &Path, rel: &str) -> Result<PathBuf> {
    let deny = |why: &str| reject::<PathBuf>(ReasonCode::AclDenied, format!("path rejected: {why}"));
    if rel.is_empty() || rel.len() > 240 {
        return deny("empty or too long");
    }
    if rel.contains('\0') || rel.contains(':') || rel.starts_with('/') || rel.starts_with('\\') || rel.contains("\\\\") {
        return deny("absolute, drive, UNC or stream syntax");
    }
    let rel_path = Path::new(rel);
    let mut cur = root.to_path_buf();
    for comp in rel_path.components() {
        match comp {
            Component::Normal(c) => {
                cur.push(c);
                if let Ok(md) = std::fs::symlink_metadata(&cur) {
                    if md.file_type().is_symlink() || is_reparse_point(&md) {
                        return deny("symlink or junction");
                    }
                }
            }
            _ => return deny("traversal component"),
        }
    }
    let canon_root = root.canonicalize()?;
    let canon = cur.canonicalize().map_err(|_| CoreError::Rejected { code: ReasonCode::TargetUnknown, message: "no such file".into() })?;
    if !canon.starts_with(&canon_root) {
        return deny("escapes mount root");
    }
    Ok(canon)
}

/// Import a Markdown/text file from the agent's mount folder into its private namespace.
pub fn import_mount_file(conn: &Connection, run_id: &str, mount_root: &Path, agent_id: &str, rel: &str) -> Result<String> {
    let path = safe_join(mount_root, rel)?;
    let md = std::fs::metadata(&path)?;
    if !md.is_file() || md.len() > 1_000_000 {
        return reject(ReasonCode::RateLimited, "not a file or larger than 1 MB");
    }
    let body = std::fs::read_to_string(&path).map_err(|_| CoreError::Invalid("file is not UTF-8 text".into()))?;
    add_resource(conn, run_id, NewResource {
        namespace: &private_ns(agent_id),
        owner_id: agent_id,
        title: rel,
        body: &body,
        kind: KnowledgeKind::Document,
        visibility: ResourceVisibility::Private,
        source_id: &format!("mount:{rel}"),
    })
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AtlasMap {
    pub rooms: Vec<String>,
    pub resources: Vec<(String, String)>,
    pub experts: Vec<String>,
}

/// Width-before-depth orientation map (§9.2): names and ids only, all authorized.
pub fn atlas(conn: &Connection, run_id: &str, agent_id: &str) -> Result<AtlasMap> {
    let mut st = conn.prepare(
        "SELECT r.name || ' (' || r.kind || ')' FROM memberships m JOIN rooms r ON r.run_id=m.run_id AND r.id=m.room_id WHERE m.run_id=?1 AND m.agent_id=?2",
    )?;
    let rooms = st.query_map(params![run_id, agent_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    let resources = visible_resources(conn, run_id, &Principal::Identity(agent_id.into()), 12)?
        .into_iter()
        .map(|r| (r.id, r.title))
        .collect();
    let mut st = conn.prepare(
        "SELECT DISTINCT a.display_name FROM memberships m1 JOIN memberships m2 ON m2.run_id=m1.run_id AND m2.room_id=m1.room_id
         JOIN agents a ON a.run_id=m2.run_id AND a.id=m2.agent_id
         WHERE m1.run_id=?1 AND m1.agent_id=?2 AND m2.agent_id != ?2 AND a.status != 'ARCHIVED' LIMIT 20",
    )?;
    let experts = st.query_map(params![run_id, agent_id], |r| r.get(0))?.collect::<rusqlite::Result<Vec<String>>>()?;
    Ok(AtlasMap { rooms, resources, experts })
}
