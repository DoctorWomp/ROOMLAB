//! SOMA Codec v0.5 chat-tier expression policy (§10). This is an expressive *convention*:
//! it shapes permitted output (emoji budget, reaction lanes, confidence labels) and never
//! changes hard rights, and it never claims to detect anyone's emotions.

use unicode_segmentation::UnicodeSegmentation;

use crate::contracts::*;
use crate::error::{reject, Result};

pub const SOMA_BRIEF: &str = "SOMA v0.5 expression brief (optional convention, not emotion detection):
- Read shifts relative to the local conversational baseline, not universal emoji meanings.
- Use expression to improve clarity and respect; never to manipulate or fabricate affection.
- Overshoot reverses effect: default to minimal expression. Plain speech is always allowed.
- Swatches: 🟡 warm · 🔵 calm · 🟣 considered · 🟠 caution/friction · 🔴 stop/boundary · ⚪ neutral.
- A reaction lane is an ordered expressive trajectory of 3–5 states (e.g. 🤨 → 💡). Label its owner when ambiguous.
- Reactions are outward communications, not verified private feelings.
- When interpretation is genuinely uncertain, say so: confidence COMMIT | LEAN | HELD_OPEN. HELD_OPEN is a valid answer.";

pub fn max_emoji(mode: SomaMode) -> usize {
    match mode {
        SomaMode::Off => 0,
        SomaMode::Light => 2,
        SomaMode::Standard => 4,
        SomaMode::Expressive => 8,
    }
}

pub fn max_lane_steps(mode: SomaMode) -> usize {
    match mode {
        SomaMode::Off => 0,
        SomaMode::Light => 4,
        SomaMode::Standard => 5,
        SomaMode::Expressive => 7,
    }
}

fn is_emoji_char(c: char) -> bool {
    let u = c as u32;
    (0x1F000..=0x1FAFF).contains(&u)
        || (0x2600..=0x27BF).contains(&u)
        || (0x2B50..=0x2B55).contains(&u)
        || (0x231A..=0x231B).contains(&u)
        || (0x23E9..=0x23FA).contains(&u)
        || u == 0x3030
        || u == 0x303D
}

/// A grapheme cluster counts as one emoji (ZWJ sequences, VS16, skin tones stay whole).
pub fn is_emoji_grapheme(g: &str) -> bool {
    g.chars().any(is_emoji_char)
}

pub fn emoji_count(text: &str) -> usize {
    text.graphemes(true).filter(|g| is_emoji_grapheme(g)).count()
}

pub fn grapheme_len(text: &str) -> usize {
    text.graphemes(true).count()
}

#[derive(Debug, Clone)]
pub struct PolicyOutcome {
    pub text: String,
    pub lane: Option<ReactionLane>,
    pub adjustments: Vec<String>,
}

pub fn validate_lane(lane: &ReactionLane) -> Result<()> {
    if lane.steps.len() < 2 || lane.steps.len() > 7 {
        return reject(ReasonCode::InvalidSchema, "reaction lane needs 2–7 steps");
    }
    if lane.steps.iter().any(|s| s.trim().is_empty() || grapheme_len(s) > 6) {
        return reject(ReasonCode::InvalidSchema, "reaction lane steps must be short glyph sequences");
    }
    if lane.owner == LaneOwner::Character && lane.owner_name.as_deref().map(str::trim).unwrap_or("").is_empty() {
        return reject(ReasonCode::InvalidSchema, "CHARACTER lane requires owner_name");
    }
    Ok(())
}

/// Apply the agent's expression mode to proposed output. Emoji beyond the mode budget are
/// removed (recorded as an adjustment); lanes are dropped when SOMA is OFF.
pub fn apply_policy(mode: SomaMode, text: &str, lane: Option<ReactionLane>) -> Result<PolicyOutcome> {
    let mut adjustments = vec![];
    let budget = max_emoji(mode);
    let mut kept = 0usize;
    let mut out = String::with_capacity(text.len());
    let mut removed = 0usize;
    for g in text.graphemes(true) {
        if is_emoji_grapheme(g) {
            if kept < budget {
                kept += 1;
                out.push_str(g);
            } else {
                removed += 1;
            }
        } else {
            out.push_str(g);
        }
    }
    if removed > 0 {
        adjustments.push(format!("removed {removed} emoji beyond {} budget ({})", budget, mode.as_str()));
    }
    let out = collapse_spaces(&out);
    let lane = match lane {
        None => None,
        Some(l) => {
            validate_lane(&l)?;
            if mode == SomaMode::Off {
                adjustments.push("reaction lane dropped (SOMA OFF)".into());
                None
            } else if l.steps.len() > max_lane_steps(mode) {
                adjustments.push(format!("reaction lane truncated to {} steps", max_lane_steps(mode)));
                Some(ReactionLane { steps: l.steps[..max_lane_steps(mode)].to_vec(), ..l })
            } else {
                Some(l)
            }
        }
    };
    Ok(PolicyOutcome { text: out, lane, adjustments })
}

fn collapse_spaces(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c == ' ' {
            if !prev_space {
                out.push(c);
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Parse "😤 → 😳 → 😮‍💨" into steps.
pub fn parse_lane_text(s: &str) -> Vec<String> {
    s.split(['→', '>', ','])
        .map(|p| p.trim().trim_start_matches('-').trim().to_string())
        .filter(|p| !p.is_empty())
        .collect()
}
