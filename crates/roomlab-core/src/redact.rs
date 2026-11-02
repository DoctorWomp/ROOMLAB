//! Secret redaction for logs and exports (T12, T58). Defence in depth: secrets are never
//! stored in the DB/settings, and any text leaving the app is scrubbed again here.

use std::io::Write;
use std::path::Path;

pub fn redact(text: &str, secrets: &[String]) -> String {
    let mut out = text.to_string();
    for s in secrets {
        if s.len() >= 4 {
            out = out.replace(s.as_str(), "[REDACTED]");
        }
    }
    // Common key shapes even if not registered as secrets.
    out = redact_prefixed(&out, "sk-", 16);
    out = redact_prefixed(&out, "Bearer ", 12);
    out
}

fn redact_prefixed(text: &str, prefix: &str, min_len: usize) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(i) = rest.find(prefix) {
        out.push_str(&rest[..i]);
        let after = &rest[i + prefix.len()..];
        let tok_len = after.find(|c: char| !(c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')).unwrap_or(after.len());
        if tok_len >= min_len {
            out.push_str(prefix);
            out.push_str("[REDACTED]");
        } else {
            out.push_str(&rest[i..i + prefix.len() + tok_len]);
        }
        rest = &after[tok_len..];
    }
    out.push_str(rest);
    out
}

/// Append a redacted line to logs/roomlab.log.
pub fn log_line(logs_dir: &Path, secrets: &[String], line: &str) {
    let path = logs_dir.join("roomlab.log");
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{} {}", crate::ids::now_utc(), redact(line, secrets));
    }
}
