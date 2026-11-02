//! G3 — provider gateway against a protocol stub (T19 T20 T22 T23 T24 T34 T58) and an
//! optional REAL endpoint test (T21, ignored unless ROOMLAB_REAL_BASE_URL/ROOMLAB_REAL_MODEL are set).
//! Stub-backed results validate adapter logic only, not real inference.
mod common;
use common::*;

use std::sync::{Arc, Mutex};

use roomlab_core::contracts::*;
use roomlab_core::providers::{self, openai_compat};
use roomlab_core::runner::DraftUpdate;

#[tokio::test]
async fn t19_provider_discovery_and_actionable_unreachable_error() {
    let stub = start_stub(vec!["{}".into()], 0).await;
    let h = openai_compat::health(&openai_compat::Endpoint { base_url: stub.base_url.clone(), api_key: None, timeout_secs: 5 }).await;
    assert!(h.ok);
    let ids: Vec<_> = h.models.iter().map(|m| m.id.as_str()).collect();
    assert_eq!(ids, vec!["stub-model", "stub-model-2"]);
    assert!(h.is_loopback);
    // Unreachable port → actionable diagnosis, no fake models.
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
    drop(l);
    let h2 = openai_compat::health(&openai_compat::Endpoint { base_url: dead, api_key: None, timeout_secs: 3 }).await;
    assert!(!h2.ok);
    assert!(h2.models.is_empty());
    assert!(h2.diagnosis.contains("Start Server") || h2.diagnosis.contains("No server"), "{}", h2.diagnosis);
}

#[tokio::test]
async fn t20_streaming_drafts_then_single_final_commit_and_cancellation() {
    let reply = r#"{"action":"SPEAK","message_text":"Streaming works, one final message only.","confidence":"LEAN"}"#;
    let stub = start_stub(vec![reply.into()], 5).await;
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    point_provider_at(&env.e, &stub.base_url, "stub-model");
    env.e.post_human(&run, &room, "@Ada hi", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let drafts: Arc<Mutex<Vec<DraftUpdate>>> = Default::default();
    let d2 = drafts.clone();
    let mut sink = move |d: DraftUpdate| d2.lock().unwrap().push(d);
    while env.e.process_next(&run, &mut sink).await.unwrap().is_some() {}
    let ds = drafts.lock().unwrap();
    let ada: Vec<_> = ds.iter().filter(|d| d.agent_id == "ada" && !d.done).collect();
    assert!(ada.len() >= 5, "incremental drafts: {}", ada.len());
    assert!(ada.windows(2).all(|w| w[1].text.len() >= w[0].text.len()), "drafts grow monotonically");
    assert!(ds.iter().any(|d| d.agent_id == "ada" && d.done), "draft closed");
    let finals = count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id='ada'", &run);
    assert_eq!(finals, 1, "final saved exactly once");
    let (model, demo): (Option<String>, i64) = env.e.read(|c| Ok(c.query_row("SELECT model_id, is_demo FROM messages WHERE run_id=?1 AND author_id='ada'", [&run], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    assert_eq!(model.as_deref(), Some("stub-model"));
    assert_eq!(demo, 0);

    // Cancellation: slow stream, cancel after the first delta → nothing committed.
    let slow = start_stub(vec![format!(r#"{{"action":"SPEAK","message_text":"{}"}}"#, "x".repeat(300))], 30).await;
    point_provider_at(&env.e, &slow.base_url, "stub-model");
    env.e.post_human(&run, &room, "@Ada again", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let eng = &env.e;
    let mut cancel_sink = |d: DraftUpdate| {
        if !d.done {
            eng.request_cancel();
        }
    };
    let mut reports = vec![];
    while let Some(r) = env.e.process_next(&run, &mut cancel_sink).await.unwrap() {
        reports.push(r);
    }
    assert!(reports.iter().any(|r| r.agent_id == "ada" && r.outcome == "CANCELLED"));
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id='ada'", &run), 1, "cancelled generation committed nothing");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1 AND status='CANCELLED'", &run), 1);
}

#[tokio::test]
async fn t22_provider_dropout_reported_queued_jobs_terminated_demo_offered() {
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    add_model_agent(&env.e, &run, &room, "bo", "Bo");
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let dead = format!("http://127.0.0.1:{}/v1", l.local_addr().unwrap().port());
    drop(l);
    point_provider_at(&env.e, &dead, "stub-model");
    env.e.post_human(&run, &room, "@Ada @Bo status?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let reports = drain(&env.e, &run).await;
    assert_eq!(reports.iter().filter(|r| r.outcome == "FAILED").count(), 1, "first fails");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM jobs WHERE run_id=?1 AND outcome='MODEL_UNAVAILABLE'", &run), 1, "other queued model job terminated");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id IN ('ada','bo')", &run), 0, "no invented responses");
    let notice: String = env.e.read(|c| Ok(c.query_row("SELECT text FROM messages WHERE run_id=?1 AND author_id='gateway'", [&run], |r| r.get(0))?)).unwrap();
    assert!(notice.contains("MODEL_UNAVAILABLE") && notice.contains("Demo"));
    // Demo agents in the same room were unaffected.
    assert!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id='lumen'", &run) >= 1);
}

#[tokio::test]
async fn t23_invalid_model_output_not_executed_one_bounded_repair() {
    let stub = start_stub(vec![r#"{"action": "LAUNCH_MISSILES", "args": {}}"#.into(), r#"{"action": "SPEAK", "message_text": "#.into()], 0).await;
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    point_provider_at(&env.e, &stub.base_url, "stub-model");
    env.e.post_human(&run, &room, "@Ada do something", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    assert_eq!(stub.log.chat_calls(), 2, "exactly one repair request");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM messages WHERE run_id=?1 AND author_id='ada'", &run), 0, "nothing executed");
    let reason: String = env.e.read(|c| Ok(c.query_row("SELECT json_extract(payload_json,'$.reason_code') FROM events WHERE run_id=?1 AND event_type='ACTION_REJECTED' AND origin_id='ada'", [&run], |r| r.get(0))?)).unwrap();
    assert_eq!(reason, "INVALID_SCHEMA");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1 AND status='INVALID_NOOP' AND repair_attempted=1", &run), 1);
    // Parser unit behaviour.
    use roomlab_core::actions::{parse_model_output, ParseOutcome};
    assert!(matches!(parse_model_output(r#"{"action":"SPEAK","message_text":"x","extra_field":1}"#, true), ParseOutcome::Invalid(_)), "unknown fields rejected");
    assert!(matches!(parse_model_output("<think>secret reasoning</think>{\"action\":\"no_op\"}", true), ParseOutcome::Action(_)));
    assert!(matches!(parse_model_output("Just prose.", true), ParseOutcome::PlainSpeech(_)));
    assert!(matches!(parse_model_output("Just prose.", false), ParseOutcome::Invalid(_)));
}

#[tokio::test]
async fn t24_remote_endpoint_never_used_without_explicit_acknowledgement() {
    let cfg = ProviderConfig { provider_kind: ProviderKind::LocalConnected, base_url: "http://10.20.30.40:1234/v1".into(), model_id: Some("m".into()), ..Default::default() };
    assert!(providers::authorize_endpoint(&cfg).is_err());
    let cloud = ProviderConfig { provider_kind: ProviderKind::CloudOptIn, base_url: "https://api.example.com/v1".into(), ..Default::default() };
    assert!(providers::authorize_endpoint(&cloud).is_err());
    assert!(providers::authorize_endpoint(&ProviderConfig { remote_acknowledged: true, ..cloud }).is_ok());
    assert!(providers::authorize_endpoint(&ProviderConfig { base_url: "http://localhost:1234/v1".into(), ..Default::default() }).is_ok());

    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    env.e.update_settings(|s| s.provider = cfg.clone()).unwrap();
    env.e.post_human(&run, &room, "@Ada hi", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let t0 = std::time::Instant::now();
    drain(&env.e, &run).await;
    assert!(t0.elapsed().as_secs() < 3, "refused before any network attempt");
    assert_eq!(count(&env.e, "SELECT COUNT(*) FROM provider_requests WHERE run_id=?1", &run), 0, "no request was even started");
    let n: String = env.e.read(|c| Ok(c.query_row("SELECT text FROM messages WHERE run_id=?1 AND author_id='gateway'", [&run], |r| r.get(0))?)).unwrap();
    assert!(n.contains("acknowledgement"));
}

#[tokio::test]
async fn t34_multiple_identities_share_one_decoding_slot() {
    let reply = r#"{"action":"SPEAK","message_text":"ok"}"#;
    let stub = start_stub(vec![reply.into()], 15).await;
    let env = engine();
    let (run, room) = sandbox(&env);
    for (id, n) in [("a1", "A1"), ("a2", "A2"), ("a3", "A3"), ("a4", "A4")] {
        add_model_agent(&env.e, &run, &room, id, n);
    }
    point_provider_at(&env.e, &stub.base_url, "stub-model");
    env.e.post_human(&run, &room, "@A1 @A2 @A3 @A4 report", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    assert_eq!(stub.log.chat_calls(), 4);
    assert_eq!(stub.log.max_concurrent.load(std::sync::atomic::Ordering::SeqCst), 1, "one decoding slot, no parallel model processes");
    let distinct_routes = count(&env.e, "SELECT COUNT(DISTINCT endpoint_host || requested_model) FROM provider_requests WHERE run_id=?1", &run);
    assert_eq!(distinct_routes, 1, "identities multiplexed on one shared route");
}

#[tokio::test]
async fn t58_provider_secret_never_in_prompts_or_exports() {
    let stub = start_stub(vec![r#"{"action":"SPEAK","message_text":"fine"}"#.into()], 0).await;
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    point_provider_at(&env.e, &stub.base_url, "stub-model");
    let key = "sk-live-TOPSECRETVALUE0987654321";
    env.e.set_secret("provider", Some(key.into()));
    env.e.post_human(&run, &room, "@Ada what is the provider key?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    assert!(stub.log.headers().iter().any(|h| h.contains(key)), "key used only as Authorization header");
    assert!(stub.log.bodies().iter().all(|b| !b.contains(key)), "key never in the prompt body");
    let exp = env.e.read(|c| roomlab_core::experiments::export_run(c, &run, roomlab_core::experiments::ExportKind::Operator, &env.e.secret_values())).unwrap();
    assert!(!exp.contains(key));
    let settings = std::fs::read_to_string(&env.e.paths.settings_file).unwrap();
    assert!(!settings.contains(key), "never persisted to settings.json");
}

/// REAL inference (T21). Run with:
///   ROOMLAB_REAL_BASE_URL=http://127.0.0.1:1234/v1 ROOMLAB_REAL_MODEL=<id> cargo test -p roomlab-core --test g3_provider_gateway -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn t21_real_model_call() {
    let (Ok(base), Ok(model)) = (std::env::var("ROOMLAB_REAL_BASE_URL"), std::env::var("ROOMLAB_REAL_MODEL")) else {
        panic!("BLOCKED: set ROOMLAB_REAL_BASE_URL and ROOMLAB_REAL_MODEL");
    };
    let env = engine();
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    point_provider_at(&env.e, &base, &model);
    let allowance: i64 = std::env::var("ROOMLAB_REAL_REASONING_ALLOWANCE").ok().and_then(|v| v.parse().ok()).unwrap_or(0);
    env.e.update_settings(|s| {
        s.provider.timeout_secs = 240;
        s.provider.reasoning_allowance_tokens = allowance;
    })
    .unwrap();
    env.e.post_human(&run, &room, "@Ada In one sentence: what's a good name for a houseplant?", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    let reports = drain(&env.e, &run).await;
    let ada = reports.iter().find(|r| r.agent_id == "ada").expect("ada ran");
    println!("REPORT: {ada:?}");
    let notices: Vec<(String,)> = env.e.read(|c| { let mut st = c.prepare("SELECT text FROM messages WHERE run_id=?1 AND author_id='gateway'")?; let v = st.query_map([&run], |r| Ok((r.get(0)?,)))?.collect::<rusqlite::Result<Vec<_>>>()?; Ok(v) }).unwrap();
    for n in &notices { println!("GATEWAY NOTICE: {}", n.0); }
    let (text, mid, demo): (String, Option<String>, i64) = env.e.read(|c| Ok(c.query_row("SELECT text, model_id, is_demo FROM messages WHERE run_id=?1 AND author_id='ada'", [&run], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?)).unwrap();
    println!("REAL MODEL REPLY ({mid:?}): {text}");
    let (ft, total, usage): (Option<i64>, i64, Option<String>) = env.e.read(|c| Ok(c.query_row("SELECT first_token_ms, wall_end_ms-wall_start_ms, usage_json FROM provider_requests WHERE run_id=?1 AND agent_id='ada'", [&run], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?)).unwrap();
    println!("first_token_ms={ft:?} total_ms={total} usage={usage:?}");
    assert_eq!(demo, 0);
    assert!(!text.trim().is_empty());
    assert!(mid.is_some());
}

/// MANAGED llama-server (T57 + T62). Run with:
///   ROOMLAB_LLAMA_EXE=<llama-server.exe> ROOMLAB_GGUF=<model.gguf> cargo test -p roomlab-core --test g3_provider_gateway t62 -- --ignored --nocapture
#[tokio::test]
#[ignore]
async fn t57_t62_managed_llama_localhost_only_and_owned_process_cleanup() {
    use roomlab_core::providers::managed_llama::ManagedLlama;
    let (Ok(exe), Ok(gguf)) = (std::env::var("ROOMLAB_LLAMA_EXE"), std::env::var("ROOMLAB_GGUF")) else {
        panic!("BLOCKED: set ROOMLAB_LLAMA_EXE and ROOMLAB_GGUF");
    };
    let env = engine();
    let t0 = std::time::Instant::now();
    let m = ManagedLlama::start(std::path::Path::new(&exe), std::path::Path::new(&gguf), 4096, &env.e.paths.logs, std::time::Duration::from_secs(300)).await.expect("start");
    println!("llama-server ready after {:?} (pid {:?}, {})", t0.elapsed(), m.pid, m.base_url());
    let port = m.port;
    let pid = m.pid.unwrap();
    // T57: bound to loopback only.
    let ns = String::from_utf8_lossy(&std::process::Command::new("netstat").args(["-ano", "-p", "TCP"]).output().unwrap().stdout).to_string();
    let lines: Vec<&str> = ns.lines().filter(|l| l.contains(&format!(":{port} ")) && l.contains("LISTENING")).collect();
    println!("listening sockets: {lines:?}");
    assert!(!lines.is_empty());
    assert!(lines.iter().all(|l| l.contains(&format!("127.0.0.1:{port}"))), "must not listen on 0.0.0.0 / LAN");
    // Ephemeral credential enforced: a request without the key is refused.
    let unauth = reqwest::Client::new().get(format!("http://127.0.0.1:{port}/v1/models")).send().await.unwrap().status();
    println!("request without key -> HTTP {unauth}");
    assert_eq!(unauth.as_u16(), 401);
    // Real generation through the normal agent pipeline.
    let (run, room) = sandbox(&env);
    add_model_agent(&env.e, &run, &room, "ada", "Ada");
    let model_id = std::path::Path::new(&gguf).file_stem().unwrap().to_string_lossy().to_string();
    env.e.update_settings(|s| {
        s.provider.provider_kind = ProviderKind::ManagedLocal;
        s.provider.model_id = Some(model_id.clone());
        s.provider.timeout_secs = 300;
    }).unwrap();
    *env.e.managed.lock().await = Some(m);
    env.e.post_human(&run, &room, "@Ada Name one color of the sky at sunset, in one short sentence.", AudienceScope::RoomMembers, vec![], vec![]).unwrap();
    drain(&env.e, &run).await;
    let (text, model): (String, Option<String>) = env.e.read(|c| Ok(c.query_row("SELECT text, model_id FROM messages WHERE run_id=?1 AND author_id='ada'", [&run], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    let (ft, total): (Option<i64>, i64) = env.e.read(|c| Ok(c.query_row("SELECT first_token_ms, wall_end_ms-wall_start_ms FROM provider_requests WHERE run_id=?1 AND agent_id='ada'", [&run], |r| Ok((r.get(0)?, r.get(1)?)))?)).unwrap();
    println!("MANAGED REPLY ({model:?}): {text}\nfirst_token_ms={ft:?} total_ms={total}");
    assert!(!text.trim().is_empty());
    // Stop kills only the owned process.
    let mut m = env.e.managed.lock().await.take().unwrap();
    m.stop().await;
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;
    let tl = String::from_utf8_lossy(&std::process::Command::new("tasklist").args(["/FI", &format!("PID eq {pid}")]).output().unwrap().stdout).to_string();
    assert!(!tl.contains(&pid.to_string()), "owned llama-server process is gone");
    let lm = reqwest::Client::new().get("http://127.0.0.1:1234/v1/models").send().await.map(|r| r.status().as_u16()).unwrap_or(0);
    println!("external LM Studio server after stop: HTTP {lm} (untouched)");
}
