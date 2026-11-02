//! Managed llama.cpp `llama-server` sidecar supervisor (§14.4). Starts an *owned* process
//! bound to 127.0.0.1 on a free port with an ephemeral API key, monitors health, and kills
//! only the process it started. External servers are never terminated.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde::Serialize;

use crate::error::{CoreError, Result};

pub struct ManagedLlama {
    child: Option<tokio::process::Child>,
    pub pid: Option<u32>,
    pub port: u16,
    pub api_key: String,
    pub model_path: PathBuf,
    pub exe_path: PathBuf,
    pub started_ms: i64,
    pub ready_ms: Option<i64>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ManagedStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub base_url: Option<String>,
    pub model_path: Option<String>,
    pub bound_host: String,
    pub ready_after_ms: Option<i64>,
}

pub fn validate_model(path: &Path) -> Result<u64> {
    let md = std::fs::metadata(path).map_err(|_| CoreError::Invalid(format!("model file not found: {}", path.display())))?;
    if !md.is_file() {
        return Err(CoreError::Invalid("model path is not a file".into()));
    }
    if path.extension().and_then(|e| e.to_str()).map(|e| e.eq_ignore_ascii_case("gguf")) != Some(true) {
        return Err(CoreError::Invalid("model must be a .gguf file".into()));
    }
    let mut magic = [0u8; 4];
    use std::io::Read;
    std::fs::File::open(path)?.read_exact(&mut magic)?;
    if &magic != b"GGUF" {
        return Err(CoreError::Invalid("file does not start with the GGUF magic header".into()));
    }
    Ok(md.len())
}

pub fn validate_exe(path: &Path) -> Result<()> {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("").to_lowercase();
    if !(name == "llama-server.exe" || name == "llama-server") {
        return Err(CoreError::Invalid("expected a llama-server executable".into()));
    }
    if !path.is_file() {
        return Err(CoreError::Invalid(format!("llama-server not found at {}", path.display())));
    }
    Ok(())
}

fn free_port() -> Result<u16> {
    let l = std::net::TcpListener::bind("127.0.0.1:0")?;
    Ok(l.local_addr()?.port())
}

impl ManagedLlama {
    pub async fn start(exe: &Path, model: &Path, ctx_size: u32, log_dir: &Path, ready_timeout: Duration) -> Result<ManagedLlama> {
        validate_exe(exe)?;
        validate_model(model)?;
        let port = free_port()?;
        let api_key = format!("rl-{}", uuid::Uuid::new_v4().simple());
        let log = std::fs::File::create(log_dir.join("llama-server.log"))?;
        let mut cmd = tokio::process::Command::new(exe);
        cmd.arg("-m").arg(model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string(), "--api-key", &api_key, "-c", &ctx_size.to_string()])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .kill_on_drop(true);
        if let Some(dir) = exe.parent() {
            cmd.current_dir(dir);
        }
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| CoreError::Provider(format!("failed to start llama-server: {e}")))?;
        let pid = child.id();
        let mut m = ManagedLlama { child: Some(child), pid, port, api_key, model_path: model.into(), exe_path: exe.into(), started_ms: crate::ids::now_ms(), ready_ms: None };
        let t0 = Instant::now();
        let client = reqwest::Client::builder().no_proxy().timeout(Duration::from_secs(2)).build().map_err(|e| CoreError::Provider(e.to_string()))?;
        loop {
            if let Some(c) = m.child.as_mut() {
                if let Ok(Some(status)) = c.try_wait() {
                    m.child = None;
                    return Err(CoreError::Provider(format!("llama-server exited during startup ({status}); see logs/llama-server.log")));
                }
            }
            if let Ok(r) = client.get(format!("http://127.0.0.1:{port}/health")).send().await {
                if r.status().is_success() {
                    m.ready_ms = Some(t0.elapsed().as_millis() as i64);
                    return Ok(m);
                }
            }
            if t0.elapsed() > ready_timeout {
                m.stop().await;
                return Err(CoreError::Provider("llama-server did not become ready in time; stopped it".into()));
            }
            tokio::time::sleep(Duration::from_millis(400)).await;
        }
    }

    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/v1", self.port)
    }

    pub fn is_running(&mut self) -> bool {
        match self.child.as_mut() {
            Some(c) => matches!(c.try_wait(), Ok(None)),
            None => false,
        }
    }

    /// Kill only the process this supervisor started.
    pub async fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill().await;
            let _ = c.wait().await;
        }
    }

    pub fn status(&mut self) -> ManagedStatus {
        let running = self.is_running();
        ManagedStatus {
            running,
            pid: if running { self.pid } else { None },
            base_url: running.then(|| self.base_url()),
            model_path: Some(self.model_path.display().to_string()),
            bound_host: "127.0.0.1".into(),
            ready_after_ms: self.ready_ms,
        }
    }
}
