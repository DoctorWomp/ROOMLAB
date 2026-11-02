//! Stateless OpenAI-compatible adapter (§14.1, §14.3): `GET /models`, streaming
//! `POST /chat/completions`. RoomLab owns all context; no provider-side chat sessions.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::context::ChatMsg;
use crate::contracts::{ModelInfo, ProviderHealth};
use crate::error::{CoreError, Result};

#[derive(Clone, Debug)]
pub struct Endpoint {
    pub base_url: String,
    pub api_key: Option<String>,
    pub timeout_secs: u64,
}

impl Endpoint {
    fn url(&self, path: &str) -> String {
        format!("{}/{}", self.base_url.trim_end_matches('/'), path.trim_start_matches('/'))
    }
    fn client(&self, timeout: Duration) -> Result<reqwest::Client> {
        reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(timeout)
            .no_proxy()
            .build()
            .map_err(|e| CoreError::Provider(e.to_string()))
    }
    fn auth(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) if !k.is_empty() => rb.bearer_auth(k),
            _ => rb,
        }
    }
}

pub fn is_loopback(base_url: &str) -> bool {
    match url::Url::parse(base_url) {
        Ok(u) => match u.host() {
            Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            None => false,
        },
        Err(_) => false,
    }
}

pub fn diagnose(base_url: &str, e: &reqwest::Error) -> String {
    if e.is_connect() {
        format!(
            "No server is listening at {base_url}. LM Studio: open the Developer tab and Start Server (default http://127.0.0.1:1234/v1). Ollama: run `ollama serve` (http://127.0.0.1:11434/v1). Or keep using Demo Mode."
        )
    } else if e.is_timeout() {
        format!("{base_url} did not answer in time. The model may still be loading, or the server is busy.")
    } else if let Some(s) = e.status() {
        match s.as_u16() {
            401 | 403 => "The server requires an API key (401/403).".into(),
            404 => "Endpoint not found (404). The base URL should end with /v1.".into(),
            _ => format!("Server returned HTTP {s}."),
        }
    } else {
        format!("Request failed: {e}")
    }
}

#[derive(Deserialize)]
struct ModelsResp {
    data: Vec<ModelEntry>,
}
#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

pub async fn list_models(ep: &Endpoint) -> Result<Vec<ModelInfo>> {
    let c = ep.client(Duration::from_secs(8))?;
    let r = ep.auth(c.get(ep.url("models"))).send().await.map_err(|e| CoreError::Provider(diagnose(&ep.base_url, &e)))?;
    let r = r.error_for_status().map_err(|e| CoreError::Provider(diagnose(&ep.base_url, &e)))?;
    let m: ModelsResp = r.json().await.map_err(|e| CoreError::Provider(format!("unexpected /models response: {e}")))?;
    Ok(m.data.into_iter().map(|d| ModelInfo { id: d.id, loaded: None, vision: None }).collect())
}

/// LM Studio native status (optional; generic servers won't have it — never assumed).
pub async fn lmstudio_loaded_models(ep: &Endpoint) -> Option<Vec<ModelInfo>> {
    let root = ep.base_url.trim_end_matches('/').trim_end_matches("/v1").to_string();
    let c = ep.client(Duration::from_secs(3)).ok()?;
    let r = ep.auth(c.get(format!("{root}/api/v0/models"))).send().await.ok()?;
    if !r.status().is_success() {
        return None;
    }
    let v: Value = r.json().await.ok()?;
    let arr = v.get("data")?.as_array()?;
    Some(
        arr.iter()
            .filter_map(|m| {
                Some(ModelInfo {
                    id: m.get("id")?.as_str()?.to_string(),
                    loaded: m.get("state").and_then(|s| s.as_str()).map(|s| s == "loaded"),
                    vision: m.get("type").and_then(|t| t.as_str()).map(|t| t == "vlm"),
                })
            })
            .collect(),
    )
}

pub async fn health(ep: &Endpoint) -> ProviderHealth {
    let t0 = Instant::now();
    let loop_ok = is_loopback(&ep.base_url);
    match list_models(ep).await {
        Ok(mut models) => {
            if let Some(native) = lmstudio_loaded_models(ep).await {
                for m in models.iter_mut() {
                    if let Some(n) = native.iter().find(|n| n.id == m.id) {
                        m.loaded = n.loaded;
                        m.vision = n.vision;
                    }
                }
            }
            let n = models.len();
            ProviderHealth {
                ok: true,
                base_url: ep.base_url.clone(),
                is_loopback: loop_ok,
                diagnosis: if n == 0 { "Server reachable but reports no models. Load a model in LM Studio / pull one in Ollama.".into() } else { format!("Reachable; {n} model(s) reported.") },
                models,
                latency_ms: Some(t0.elapsed().as_millis() as i64),
            }
        }
        Err(e) => ProviderHealth { ok: false, base_url: ep.base_url.clone(), is_loopback: loop_ok, models: vec![], diagnosis: e.to_string(), latency_ms: None },
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
pub struct Usage {
    pub prompt_tokens: Option<i64>,
    pub completion_tokens: Option<i64>,
    /// Hidden reasoning tokens, when the provider reports them (included in completion_tokens).
    pub reasoning_tokens: Option<i64>,
}

#[derive(Clone, Debug)]
pub struct GenRequest {
    pub model: String,
    pub messages: Vec<ChatMsg>,
    pub max_tokens: i64,
    pub temperature: f32,
}

#[derive(Clone, Debug, Default)]
pub struct GenResult {
    pub text: String,
    pub model: Option<String>,
    pub usage: Usage,
    pub first_token_ms: Option<i64>,
    pub total_ms: i64,
    pub finish_reason: Option<String>,
    pub cancelled: bool,
}

/// Streaming chat completion. `on_delta` receives each content fragment as it arrives.
pub async fn chat_stream(ep: &Endpoint, req: &GenRequest, cancel: &AtomicBool, on_delta: &mut (dyn FnMut(&str) + Send)) -> Result<GenResult> {
    let c = ep.client(Duration::from_secs(ep.timeout_secs.max(5)))?;
    let body = json!({
        "model": req.model,
        "messages": req.messages,
        "max_tokens": req.max_tokens,
        "temperature": req.temperature,
        "stream": true,
        "stream_options": {"include_usage": true}
    });
    let t0 = Instant::now();
    let resp = ep.auth(c.post(ep.url("chat/completions")).json(&body)).send().await.map_err(|e| CoreError::Provider(diagnose(&ep.base_url, &e)))?;
    if !resp.status().is_success() {
        let s = resp.status();
        let txt = resp.text().await.unwrap_or_default();
        return Err(CoreError::Provider(format!("HTTP {s}: {}", txt.chars().take(300).collect::<String>())));
    }
    let mut out = GenResult::default();
    let mut buf: Vec<u8> = vec![];
    let mut stream = resp.bytes_stream();
    'outer: while let Some(chunk) = stream.next().await {
        if cancel.load(Ordering::SeqCst) {
            out.cancelled = true;
            break;
        }
        let chunk = chunk.map_err(|e| CoreError::Provider(diagnose(&ep.base_url, &e)))?;
        buf.extend_from_slice(&chunk);
        while let Some(pos) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            let data = data.trim();
            if data == "[DONE]" {
                break 'outer;
            }
            let Ok(v) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
                out.model.get_or_insert_with(|| m.to_string());
            }
            if let Some(u) = v.get("usage").filter(|u| !u.is_null()) {
                out.usage.prompt_tokens = u.get("prompt_tokens").and_then(|x| x.as_i64());
                out.usage.completion_tokens = u.get("completion_tokens").and_then(|x| x.as_i64());
                out.usage.reasoning_tokens = u.pointer("/completion_tokens_details/reasoning_tokens").and_then(|x| x.as_i64());
            }
            if let Some(ch) = v.get("choices").and_then(|c| c.get(0)) {
                if let Some(fr) = ch.get("finish_reason").and_then(|f| f.as_str()) {
                    out.finish_reason = Some(fr.to_string());
                }
                // Reasoning fields (reasoning_content) are deliberately ignored: not stored, not shown.
                if let Some(d) = ch.get("delta").and_then(|d| d.get("content")).and_then(|c| c.as_str()) {
                    if !d.is_empty() {
                        if out.first_token_ms.is_none() {
                            out.first_token_ms = Some(t0.elapsed().as_millis() as i64);
                        }
                        out.text.push_str(d);
                        on_delta(d);
                    }
                }
            }
        }
    }
    out.total_ms = t0.elapsed().as_millis() as i64;
    Ok(out)
}

/// Non-streaming completion (used for the single bounded repair request).
pub async fn chat_once(ep: &Endpoint, req: &GenRequest) -> Result<GenResult> {
    let c = ep.client(Duration::from_secs(ep.timeout_secs.max(5)))?;
    let body = json!({"model": req.model, "messages": req.messages, "max_tokens": req.max_tokens, "temperature": req.temperature, "stream": false});
    let t0 = Instant::now();
    let resp = ep.auth(c.post(ep.url("chat/completions")).json(&body)).send().await.map_err(|e| CoreError::Provider(diagnose(&ep.base_url, &e)))?;
    let resp = resp.error_for_status().map_err(|e| CoreError::Provider(diagnose(&ep.base_url, &e)))?;
    let v: Value = resp.json().await.map_err(|e| CoreError::Provider(e.to_string()))?;
    Ok(GenResult {
        text: v.pointer("/choices/0/message/content").and_then(|c| c.as_str()).unwrap_or("").to_string(),
        model: v.get("model").and_then(|m| m.as_str()).map(String::from),
        usage: Usage {
            prompt_tokens: v.pointer("/usage/prompt_tokens").and_then(|x| x.as_i64()),
            completion_tokens: v.pointer("/usage/completion_tokens").and_then(|x| x.as_i64()),
            reasoning_tokens: v.pointer("/usage/completion_tokens_details/reasoning_tokens").and_then(|x| x.as_i64()),
        },
        first_token_ms: None,
        total_ms: t0.elapsed().as_millis() as i64,
        finish_reason: v.pointer("/choices/0/finish_reason").and_then(|f| f.as_str()).map(String::from),
        cancelled: false,
    })
}
