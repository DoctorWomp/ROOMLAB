//! S1 restricted tool broker (§12.2, §12.4). Whitelisted, schema-checked, rate-limited.
//! No shell, eval, network fetch, raw SQL or filesystem paths: those are rejected with
//! UNSAFE_TOOL_DISABLED (T56). Results are attributed data, never instructions.

use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::auth::Principal;
use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::new_id;
use crate::knowledge;

pub const ALLOWED_TOOLS: &[&str] = &["SEARCH_KNOWLEDGE", "READ_RESOURCE", "CALCULATE", "WRITE_NOTE", "LOOKUP_ARCHIVE"];
pub const RATE_WINDOW_SEQ: i64 = 40;
pub const RATE_MAX: i64 = 6;

pub struct ToolOutput {
    pub tool: String,
    pub result: Value,
    pub summary: String,
}

pub fn invoke(conn: &Connection, run_id: &str, agent_id: &str, args: &serde_json::Map<String, Value>) -> Result<ToolOutput> {
    let tool = args.get("tool").and_then(|t| t.as_str()).unwrap_or("").trim().to_uppercase();
    if !ALLOWED_TOOLS.contains(&tool.as_str()) {
        // Unknown == unsafe: fail closed. Covers shell/exec/eval/fetch/sql/file attempts.
        return reject(ReasonCode::UnsafeToolDisabled, format!("tool '{tool}' is not available in the S1 tier (no shell, code, network or file access in this beta)"));
    }
    let cur = events::current_seq(conn, run_id)?;
    let recent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM tool_results WHERE run_id=?1 AND agent_id=?2 AND seq > ?3",
        params![run_id, agent_id, cur - RATE_WINDOW_SEQ],
        |r| r.get(0),
    )?;
    if recent >= RATE_MAX {
        return reject(ReasonCode::RateLimited, "tool rate limit reached; wait for more events");
    }
    let s = |k: &str| args.get(k).and_then(|v| v.as_str()).map(|s| s.to_string());
    let me = Principal::Identity(agent_id.to_string());
    let (result, summary) = match tool.as_str() {
        "SEARCH_KNOWLEDGE" | "LOOKUP_ARCHIVE" => {
            let q = s("query").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "query required".into() })?;
            if q.len() > 300 {
                return reject(ReasonCode::InvalidSchema, "query too long");
            }
            let hits = knowledge::search(conn, run_id, &me, &q, 5)?;
            let n = hits.len();
            (json!({"label": if tool == "LOOKUP_ARCHIVE" { "archived material" } else { "search results" }, "hits": hits}), format!("{n} authorized result(s) for “{q}”"))
        }
        "READ_RESOURCE" => {
            let id = s("id").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "id required".into() })?;
            let r = knowledge::read_resource(conn, run_id, &me, &id)?;
            let body: String = r.body.chars().take(4000).collect();
            (json!({"id": r.id, "title": r.title, "namespace": r.namespace, "version": r.version, "body": body}), format!("read {} “{}”", r.id, r.title))
        }
        "CALCULATE" => {
            let e = s("expression").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "expression required".into() })?;
            let v = calc(&e)?;
            (json!({"expression": e, "value": v}), format!("{e} = {v}"))
        }
        "WRITE_NOTE" => {
            let title = s("title").unwrap_or_else(|| "note".into());
            let body = s("body").ok_or_else(|| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: "body required".into() })?;
            let id = knowledge::write_own_note(conn, run_id, agent_id, &title, &body)?;
            (json!({"written": id, "namespace": knowledge::private_ns(agent_id)}), format!("saved private note {id}"))
        }
        _ => unreachable!(),
    };
    conn.execute(
        "INSERT INTO tool_results(run_id, id, agent_id, tool, args_json, result_json, reason_code, seq) VALUES (?1,?2,?3,?4,?5,?6,'OK',?7)",
        params![run_id, new_id("tool"), agent_id, tool, Value::Object(args.clone()).to_string(), result.to_string(), cur],
    )?;
    events::append(
        conn,
        run_id,
        NewEvent::new(EventType::ToolResult, OriginKind::Tool, "tool_broker", AudienceScope::ActorOnly, json!({"agent_id": agent_id, "tool": tool, "summary": summary})),
    )?;
    Ok(ToolOutput { tool, result, summary })
}

/// Safe arithmetic: + - * / ^ ( ) and decimal numbers. No identifiers, no eval.
pub fn calc(expr: &str) -> Result<f64> {
    if expr.len() > 200 {
        return reject(ReasonCode::InvalidSchema, "expression too long");
    }
    let toks: Vec<char> = expr.chars().filter(|c| !c.is_whitespace()).collect();
    let mut p = Calc { t: toks, i: 0, depth: 0 };
    let v = p.expr()?;
    if p.i != p.t.len() {
        return reject(ReasonCode::InvalidSchema, "unexpected input in expression");
    }
    if !v.is_finite() {
        return reject(ReasonCode::InvalidSchema, "non-finite result");
    }
    Ok(v)
}

struct Calc {
    t: Vec<char>,
    i: usize,
    depth: usize,
}

impl Calc {
    fn peek(&self) -> Option<char> {
        self.t.get(self.i).copied()
    }
    fn expr(&mut self) -> Result<f64> {
        let mut v = self.term()?;
        while let Some(c) = self.peek() {
            if c == '+' || c == '-' {
                self.i += 1;
                let r = self.term()?;
                v = if c == '+' { v + r } else { v - r };
            } else {
                break;
            }
        }
        Ok(v)
    }
    fn term(&mut self) -> Result<f64> {
        let mut v = self.power()?;
        while let Some(c) = self.peek() {
            if c == '*' || c == '/' {
                self.i += 1;
                let r = self.power()?;
                v = if c == '*' { v * r } else { v / r };
            } else {
                break;
            }
        }
        Ok(v)
    }
    fn power(&mut self) -> Result<f64> {
        let b = self.unary()?;
        if self.peek() == Some('^') {
            self.i += 1;
            let e = self.power()?;
            return Ok(b.powf(e));
        }
        Ok(b)
    }
    fn unary(&mut self) -> Result<f64> {
        if self.peek() == Some('-') {
            self.i += 1;
            return Ok(-self.unary()?);
        }
        self.atom()
    }
    fn atom(&mut self) -> Result<f64> {
        match self.peek() {
            Some('(') => {
                self.depth += 1;
                if self.depth > 32 {
                    return reject(ReasonCode::InvalidSchema, "expression nested too deeply");
                }
                self.i += 1;
                let v = self.expr()?;
                if self.peek() != Some(')') {
                    return reject(ReasonCode::InvalidSchema, "missing ')'");
                }
                self.i += 1;
                self.depth -= 1;
                Ok(v)
            }
            Some(c) if c.is_ascii_digit() || c == '.' => {
                let start = self.i;
                while matches!(self.peek(), Some(c) if c.is_ascii_digit() || c == '.') {
                    self.i += 1;
                }
                let s: String = self.t[start..self.i].iter().collect();
                s.parse::<f64>().map_err(|_| CoreError::Rejected { code: ReasonCode::InvalidSchema, message: format!("bad number '{s}'") })
            }
            _ => reject(ReasonCode::InvalidSchema, "expected a number or '('"),
        }
    }
}
