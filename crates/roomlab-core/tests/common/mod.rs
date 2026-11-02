//! Shared test helpers: temp engines and a protocol stub of an OpenAI-compatible server.
//! The stub is a real TCP/HTTP server so the production reqwest/SSE code path is exercised.
//! Stub-backed tests validate adapter logic only; they are not evidence of real inference.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use roomlab_core::contracts::*;
use roomlab_core::runner::{DraftUpdate, JobReport};
use roomlab_core::Engine;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

pub struct TestEnv {
    pub dir: tempfile::TempDir,
    pub e: Engine,
}

pub fn engine() -> TestEnv {
    let dir = tempfile::tempdir().unwrap();
    let e = Engine::open_temp(dir.path()).unwrap();
    TestEnv { dir, e }
}

/// Sandbox run with a Commons room ("commons") and lumen/moth/sable.
pub fn sandbox(env: &TestEnv) -> (String, String) {
    let run = env.e.first_run("DEMO").unwrap();
    let room = env.e.read(|c| Ok(roomlab_core::room::list_rooms(c, &run)?[0].id.clone())).unwrap();
    (run, room)
}

pub async fn drain(e: &Engine, run: &str) -> Vec<JobReport> {
    let mut v = vec![];
    let mut sink = |_d: DraftUpdate| {};
    while let Some(r) = e.process_next(run, &mut sink).await.unwrap() {
        v.push(r);
        assert!(v.len() < 400, "runaway job loop");
    }
    v
}

pub fn count(e: &Engine, sql: &str, run: &str) -> i64 {
    e.read(|c| Ok(c.query_row(sql, [run], |r| r.get(0))?)).unwrap()
}

pub fn messages_text(e: &Engine, run: &str, room: &str) -> Vec<(String, String)> {
    e.read(|c| {
        let mut st = c.prepare("SELECT author_id, text FROM messages WHERE run_id=?1 AND room_id=?2 ORDER BY seq")?;
        let v = st.query_map([run, room], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(v)
    })
    .unwrap()
}

/// Make an L1 (model-backed) agent on the shared `local/default` route and add it to `room`.
pub fn add_model_agent(e: &Engine, run: &str, room: &str, id: &str, name: &str) {
    e.tx(|c| {
        let p = roomlab_core::agent::profile_from_seed(&roomlab_core::agent::ProfileSeed {
            id, name, icon: "🤖", role: "participant", persona: "A concise participant.", directive: "Reply briefly.",
            cognition: Cognition::L1Reactive, route: "local/default", soma: SomaMode::Light, demo_policy: None,
            actions: roomlab_core::agent::CHAT_ACTIONS.to_vec(), motivation: "help", style: "concise",
        });
        roomlab_core::agent::create_agent(c, run, &p, AgentKind::Agent, None, None)?;
        roomlab_core::room::add_member(c, run, room, id, "member", "ROOM")
    })
    .unwrap();
}

pub fn point_provider_at(e: &Engine, base: &str, model: &str) {
    e.update_settings(|s| {
        s.provider.provider_kind = ProviderKind::LocalConnected;
        s.provider.base_url = base.to_string();
        s.provider.model_id = Some(model.to_string());
        s.provider.timeout_secs = 20;
    })
    .unwrap();
}

#[derive(Clone, Default)]
pub struct StubLog {
    pub requests: Arc<Mutex<Vec<(String, String, String)>>>, // (request line, headers, body)
    pub max_concurrent: Arc<AtomicUsize>,
    current: Arc<AtomicUsize>,
    calls: Arc<AtomicUsize>,
}

impl StubLog {
    pub fn chat_calls(&self) -> usize {
        self.requests.lock().unwrap().iter().filter(|r| r.0.contains("/chat/completions")).count()
    }
    pub fn bodies(&self) -> Vec<String> {
        self.requests.lock().unwrap().iter().map(|r| r.2.clone()).collect()
    }
    pub fn headers(&self) -> Vec<String> {
        self.requests.lock().unwrap().iter().map(|r| r.1.clone()).collect()
    }
}

pub struct Stub {
    pub base_url: String,
    pub log: StubLog,
    pub handle: tokio::task::JoinHandle<()>,
}

/// Start a stub. `replies[i]` is the full text streamed for the i-th chat call (last one repeats).
pub async fn start_stub(replies: Vec<String>, chunk_delay_ms: u64) -> Stub {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let log = StubLog::default();
    let l2 = log.clone();
    let replies = Arc::new(replies);
    let handle = tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else { break };
            let log = l2.clone();
            let replies = replies.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 65536];
                let mut data = vec![];
                let (head, body) = loop {
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    data.extend_from_slice(&buf[..n]);
                    if let Some(pos) = data.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&data[..pos]).to_string();
                        let cl = head.lines().find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))).unwrap_or(0);
                        while data.len() < pos + 4 + cl {
                            let n = sock.read(&mut buf).await.unwrap_or(0);
                            if n == 0 {
                                break;
                            }
                            data.extend_from_slice(&buf[..n]);
                        }
                        let body = String::from_utf8_lossy(&data[pos + 4..]).to_string();
                        break (head, body);
                    }
                };
                let line = head.lines().next().unwrap_or("").to_string();
                log.requests.lock().unwrap().push((line.clone(), head.clone(), body.clone()));
                if line.starts_with("GET") && line.contains("/v1/models") {
                    let j = r#"{"object":"list","data":[{"id":"stub-model","object":"model"},{"id":"stub-model-2","object":"model"}]}"#;
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{j}", j.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                    return;
                }
                if line.starts_with("GET") {
                    let _ = sock.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                    return;
                }
                let idx = log.calls.fetch_add(1, Ordering::SeqCst);
                let text = replies.get(idx).or(replies.last()).cloned().unwrap_or_default();
                let cur = log.current.fetch_add(1, Ordering::SeqCst) + 1;
                log.max_concurrent.fetch_max(cur, Ordering::SeqCst);
                if body.contains("\"stream\":false") {
                    let j = serde_json::json!({"model": "stub-model", "choices": [{"message": {"role": "assistant", "content": text}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 50, "completion_tokens": 10}}).to_string();
                    let resp = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{j}", j.len());
                    let _ = sock.write_all(resp.as_bytes()).await;
                } else {
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n").await;
                    let chars: Vec<char> = text.chars().collect();
                    for chunk in chars.chunks(6) {
                        let s: String = chunk.iter().collect();
                        let ev = serde_json::json!({"model": "stub-model", "choices": [{"delta": {"content": s}, "finish_reason": null}]});
                        if sock.write_all(format!("data: {ev}\n\n").as_bytes()).await.is_err() {
                            break;
                        }
                        let _ = sock.flush().await;
                        if chunk_delay_ms > 0 {
                            tokio::time::sleep(Duration::from_millis(chunk_delay_ms)).await;
                        }
                    }
                    let fin = serde_json::json!({"model": "stub-model", "choices": [{"delta": {}, "finish_reason": "stop"}], "usage": {"prompt_tokens": 120, "completion_tokens": (chars.len() / 4).max(1)}});
                    let _ = sock.write_all(format!("data: {fin}\n\ndata: [DONE]\n\n").as_bytes()).await;
                }
                log.current.fetch_sub(1, Ordering::SeqCst);
                let _ = sock.shutdown().await;
            });
        }
    });
    Stub { base_url: format!("http://127.0.0.1:{port}/v1"), log, handle }
}
