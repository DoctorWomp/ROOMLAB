//! Image attachments (§11). Stored under a generated id with MIME sniffing, size/dimension
//! limits and a content hash. Never executed. Text-only agents get `image_attachment_present`.

use base64::Engine as _;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use crate::events::{self, NewEvent};
use crate::ids::{new_id, now_utc, sha256_hex};

pub const MAX_BYTES: usize = 8 * 1024 * 1024;
pub const MAX_DIM: u32 = 8192;

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AttachmentInfo {
    pub id: String,
    pub mime: String,
    pub width: u32,
    pub height: u32,
    pub size_bytes: i64,
    pub sha256: String,
    pub original_name: String,
}

/// Sniff magic bytes and read dimensions. Returns (mime, ext, w, h).
pub fn sniff(bytes: &[u8]) -> Option<(&'static str, &'static str, u32, u32)> {
    if bytes.len() > 24 && bytes.starts_with(b"\x89PNG\r\n\x1a\n") && &bytes[12..16] == b"IHDR" {
        let w = u32::from_be_bytes(bytes[16..20].try_into().ok()?);
        let h = u32::from_be_bytes(bytes[20..24].try_into().ok()?);
        return Some(("image/png", "png", w, h));
    }
    if bytes.len() > 4 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        // Walk JPEG segments to the first SOFn marker.
        let mut i = 2usize;
        while i + 9 < bytes.len() {
            if bytes[i] != 0xFF {
                i += 1;
                continue;
            }
            let marker = bytes[i + 1];
            if (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC {
                let h = u16::from_be_bytes([bytes[i + 5], bytes[i + 6]]) as u32;
                let w = u16::from_be_bytes([bytes[i + 7], bytes[i + 8]]) as u32;
                return Some(("image/jpeg", "jpg", w, h));
            }
            let len = u16::from_be_bytes([bytes[i + 2], bytes[i + 3]]) as usize;
            if len < 2 {
                return None;
            }
            i += 2 + len;
        }
        return None;
    }
    if bytes.len() > 30 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        let chunk = &bytes[12..16];
        let (w, h) = match chunk {
            b"VP8X" => (1 + u32::from_le_bytes([bytes[24], bytes[25], bytes[26], 0]), 1 + u32::from_le_bytes([bytes[27], bytes[28], bytes[29], 0])),
            b"VP8L" => {
                let b = &bytes[21..25];
                let bits = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                (1 + (bits & 0x3FFF), 1 + ((bits >> 14) & 0x3FFF))
            }
            b"VP8 " => (u16::from_le_bytes([bytes[26], bytes[27]]) as u32 & 0x3FFF, u16::from_le_bytes([bytes[28], bytes[29]]) as u32 & 0x3FFF),
            _ => return None,
        };
        return Some(("image/webp", "webp", w, h));
    }
    if bytes.len() > 10 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        let w = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
        let h = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
        return Some(("image/gif", "gif", w, h));
    }
    None
}

pub fn safe_name(name: &str) -> String {
    let n: String = name.chars().filter(|c| c.is_alphanumeric() || matches!(c, '.' | '-' | '_' | ' ')).take(80).collect();
    if n.trim().is_empty() { "image".into() } else { n }
}

pub fn store(conn: &Connection, dir: &std::path::Path, run_id: &str, room_id: &str, uploader: &str, original_name: &str, bytes: &[u8]) -> Result<AttachmentInfo> {
    if bytes.len() > MAX_BYTES {
        return reject(ReasonCode::RateLimited, format!("image exceeds {} MB", MAX_BYTES / 1024 / 1024));
    }
    let Some((mime, ext, w, h)) = sniff(bytes) else {
        return reject(ReasonCode::InvalidSchema, "unsupported or corrupt image (PNG, JPEG, WebP, GIF only)");
    };
    if w == 0 || h == 0 || w > MAX_DIM || h > MAX_DIM {
        return reject(ReasonCode::InvalidSchema, format!("image dimensions {w}x{h} outside limits"));
    }
    let id = new_id("att");
    let stored = format!("{id}.{ext}");
    std::fs::write(dir.join(&stored), bytes)?;
    let hash = sha256_hex(bytes);
    conn.execute(
        "INSERT INTO attachments(run_id, id, room_id, sha256, mime, width, height, size_bytes, original_name, stored_name, uploader_id, created_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
        params![run_id, id, room_id, hash, mime, w, h, bytes.len() as i64, safe_name(original_name), stored, uploader, now_utc()],
    )?;
    events::append(conn, run_id, NewEvent::new(EventType::AttachmentAdded, OriginKind::Human, uploader, AudienceScope::OperatorOnly, json!({"attachment_id": id, "mime": mime, "w": w, "h": h, "sha256": hash})).room(room_id))?;
    Ok(AttachmentInfo { id, mime: mime.into(), width: w, height: h, size_bytes: bytes.len() as i64, sha256: hash, original_name: safe_name(original_name) })
}

/// Returns a data URL if `viewer` (None = operator) observed a message carrying the attachment.
pub fn data_url(conn: &Connection, dir: &std::path::Path, run_id: &str, id: &str, viewer: Option<&str>) -> Result<String> {
    let row: Option<(String, String)> = conn
        .query_row("SELECT mime, stored_name FROM attachments WHERE run_id=?1 AND id=?2", params![run_id, id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let Some((mime, stored)) = row else { return Err(CoreError::NotFound("attachment".into())) };
    if let Some(v) = viewer {
        let ok: Option<i64> = conn
            .query_row(
                "SELECT 1 FROM messages m JOIN observations o ON o.run_id=m.run_id AND o.observer_id=?3 AND o.kind='MESSAGE' AND json_extract(o.data_json,'$.message_id')=m.message_id
                 WHERE m.run_id=?1 AND m.attachments_json LIKE '%' || ?2 || '%' LIMIT 1",
                params![run_id, id, v],
                |r| r.get(0),
            )
            .optional()?;
        if ok.is_none() {
            return Err(CoreError::NotFound("attachment".into()));
        }
    }
    if stored.contains(['/', '\\']) || stored.contains("..") {
        return Err(CoreError::Invalid("bad stored name".into()));
    }
    let bytes = std::fs::read(dir.join(stored))?;
    Ok(format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}
