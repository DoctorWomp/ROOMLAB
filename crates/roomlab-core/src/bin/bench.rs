//! Real-endpoint benchmark (§15.3, §18.6, T64). Measures actual calls only; prints raw samples
//! and p50/p95 with sample counts. Never invents numbers: if the endpoint is unreachable it
//! reports BLOCKED and exits non-zero.
//!
//! Usage: roomlab-bench --base-url http://127.0.0.1:1234/v1 --model <id> [--agents 1,3,5] [--repeats 3] [--out report.md]

use std::time::Instant;

use roomlab_core::contracts::*;
use roomlab_core::providers::openai_compat::{health, Endpoint};
use roomlab_core::Engine;

fn arg(name: &str) -> Option<String> {
    let a: Vec<String> = std::env::args().collect();
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1).cloned())
}

fn pct(v: &mut [i64], p: f64) -> Option<i64> {
    if v.is_empty() {
        return None;
    }
    v.sort();
    let idx = ((p * (v.len() as f64 - 1.0)).round() as usize).min(v.len() - 1);
    Some(v[idx])
}

#[tokio::main]
async fn main() {
    let base = arg("--base-url").unwrap_or_else(|| "http://127.0.0.1:1234/v1".into());
    let model = arg("--model").expect("--model required");
    let agents: Vec<usize> = arg("--agents").unwrap_or_else(|| "1,3,5".into()).split(',').filter_map(|s| s.trim().parse().ok()).collect();
    let repeats: usize = arg("--repeats").and_then(|r| r.parse().ok()).unwrap_or(3);
    let out = arg("--out");
    let allowance: i64 = arg("--reasoning-allowance").and_then(|r| r.parse().ok()).unwrap_or(0);
    let ep = Endpoint { base_url: base.clone(), api_key: None, timeout_secs: 300 };
    let h = health(&ep).await;
    if !h.ok {
        eprintln!("BLOCKED: {}", h.diagnosis);
        std::process::exit(2);
    }
    let mut report = format!(
        "# RoomLab measured benchmark\n\nEndpoint: {base}\nModel: {model}\nRepeats per condition: {repeats}\nReasoning allowance: {allowance} tokens\nDate (UTC): {}\nScenario: Commons, one human message mentioning every agent; each agent makes one real call (visible output cap 200 tokens + reasoning allowance, temperature 0.7). Concurrency 1 (default scheduler).\n\n| agents | repeat | calls ok | calls failed | wall for all turns (ms) |\n|---|---|---|---|---|\n",
        roomlab_core::ids::now_utc()
    );
    let mut summary = String::from("\n## Per-condition latency (actual samples)\n\n| agents | n calls | first-token p50 | first-token p95 | total p50 | total p95 | queue-wait p50 | queue-wait p95 | completion tokens p50 |\n|---|---|---|---|---|---|---|---|---|\n");
    for &n in &agents {
        let (mut ft, mut tot, mut qw, mut toks) = (vec![], vec![], vec![], vec![]);
        for rep in 0..repeats {
            let dir = tempfile_dir();
            let e = Engine::open_temp(&dir).expect("engine");
            e.update_settings(|s| {
                s.provider.provider_kind = ProviderKind::LocalConnected;
                s.provider.base_url = base.clone();
                s.provider.model_id = Some(model.clone());
                s.provider.timeout_secs = 300;
                s.provider.reasoning_allowance_tokens = allowance;
            })
            .unwrap();
            let run = e.new_run("bench", None, SessionType::Sandbox, 4201).unwrap();
            let room = e.create_room_from_template(&run, RoomKind::Commons, "bench").unwrap();
            // Replace template agents with N model-backed agents on the same route.
            let mut names = vec![];
            e.tx(|c| {
                for a in ["lumen", "moth", "sable"] {
                    roomlab_core::room::remove_member(c, &run, &room, a)?;
                }
                for i in 0..n {
                    let id = format!("bench{i}");
                    let mut p = roomlab_core::agent::profile_from_seed(&roomlab_core::agent::ProfileSeed {
                        id: &id, name: &format!("Bench{i}"), icon: "🤖", role: "participant", persona: "A concise chat participant.", directive: "Reply briefly.",
                        cognition: Cognition::L1Reactive, route: "local/default", soma: SomaMode::Light, demo_policy: None,
                        actions: roomlab_core::agent::CHAT_ACTIONS.to_vec(), motivation: "be helpful", style: "concise",
                    });
                    p.limits.turn_output_tokens = 200;
                    roomlab_core::agent::create_agent(c, &run, &p, AgentKind::Agent, None, None)?;
                    roomlab_core::room::add_member(c, &run, &room, &id, "member", "ROOM")?;
                    names.push(format!("@Bench{i}"));
                }
                Ok(())
            })
            .unwrap();
            e.post_human(&run, &room, &format!("{} quick check-in: what is one thing you'd want to know about a new teammate?", names.join(" ")), AudienceScope::RoomMembers, vec![], vec![]).unwrap();
            let t0 = Instant::now();
            let (mut ok, mut failed) = (0, 0);
            let mut sink = |_d: roomlab_core::runner::DraftUpdate| {};
            while let Some(r) = e.process_next(&run, &mut sink).await.unwrap() {
                if r.outcome == "FAILED" { failed += 1 } else { ok += 1 }
            }
            let wall = t0.elapsed().as_millis();
            report.push_str(&format!("| {n} | {} | {ok} | {failed} | {wall} |\n", rep + 1));
            e.read(|c| {
                let mut st = c.prepare("SELECT first_token_ms, wall_end_ms - wall_start_ms, queue_wait_ms, json_extract(usage_json,'$.completion_tokens') FROM provider_requests WHERE run_id=?1 AND status IN ('ACTION','PLAIN_SPEECH','EMPTY_NOOP','INVALID_NOOP')")?;
                let rows = st.query_map([&run], |r| Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, Option<i64>>(1)?, r.get::<_, Option<i64>>(2)?, r.get::<_, Option<i64>>(3)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
                for (f, t, q, k) in rows {
                    if let Some(f) = f { ft.push(f) }
                    if let Some(t) = t { tot.push(t) }
                    if let Some(q) = q { qw.push(q) }
                    if let Some(k) = k { toks.push(k) }
                }
                Ok(())
            })
            .unwrap();
            drop(e);
            let _ = std::fs::remove_dir_all(&dir);
        }
        let f = |v: Option<i64>| v.map(|x| x.to_string()).unwrap_or_else(|| "n/a".into());
        let nc = tot.len();
        summary.push_str(&format!(
            "| {n} | {nc} | {} | {} | {} | {} | {} | {} | {} |\n",
            f(pct(&mut ft, 0.5)), f(pct(&mut ft, 0.95)), f(pct(&mut tot, 0.5)), f(pct(&mut tot, 0.95)), f(pct(&mut qw, 0.5)), f(pct(&mut qw, 0.95)), f(pct(&mut toks, 0.5))
        ));
    }
    report.push_str(&summary);
    report.push_str("\nAll times in milliseconds, measured by RoomLab around real HTTP calls. Percentiles use nearest-rank on the samples listed; with fewer than ~20 samples p95 is effectively the max and should be read as indicative only. Queue wait = time from job enqueue to call start (includes earlier agents' turns because concurrency is 1). RAM/VRAM are not measured by this tool.\n");
    println!("{report}");
    if let Some(o) = out {
        std::fs::write(o, &report).expect("write report");
    }
}

fn tempfile_dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("roomlab_bench_{}", uuid_like()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn uuid_like() -> String {
    roomlab_core::ids::new_id("b")
}
