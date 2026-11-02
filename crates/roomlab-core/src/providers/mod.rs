//! Provider-neutral inference gateway (§14).

pub mod managed_llama;
pub mod openai_compat;

use crate::contracts::*;
use crate::error::{reject, CoreError, Result};
use openai_compat::{is_loopback, Endpoint};

#[derive(Clone, Debug)]
pub struct Target {
    pub kind: ProviderKind,
    pub endpoint: Endpoint,
    pub model_id: String,
    pub temperature: f32,
    pub reasoning_allowance: i64,
    pub is_loopback: bool,
}

/// Cloud/remote policy (T24): nothing leaves localhost unless explicitly acknowledged.
pub fn authorize_endpoint(cfg: &ProviderConfig) -> Result<()> {
    let loopback = is_loopback(&cfg.base_url);
    if (!loopback || cfg.provider_kind == ProviderKind::CloudOptIn) && !cfg.remote_acknowledged {
        return Err(CoreError::Forbidden(format!(
            "{} is not a loopback address. Remote/cloud endpoints require explicit operator acknowledgement that prompts will leave this computer.",
            cfg.base_url
        )));
    }
    if url::Url::parse(&cfg.base_url).map(|u| !matches!(u.scheme(), "http" | "https")).unwrap_or(true) {
        return Err(CoreError::Invalid("base URL must be http(s)".into()));
    }
    Ok(())
}

/// Resolve a logical model route to a concrete endpoint. Never substitutes silently (§14.5).
pub fn resolve(route_id: &str, cfg: &ProviderConfig, api_key: Option<String>, managed_base: Option<String>) -> Result<Target> {
    if route_id == "demo" {
        return reject(ReasonCode::ModelUnavailable, "route 'demo' has no model");
    }
    let base = match cfg.provider_kind {
        ProviderKind::Demo => return reject(ReasonCode::ModelUnavailable, "No inference provider is configured (Demo Mode). Connect LM Studio/Ollama in Settings or switch this agent to a Demo policy."),
        ProviderKind::ManagedLocal => managed_base.ok_or_else(|| CoreError::Rejected { code: ReasonCode::ModelUnavailable, message: "Managed llama-server is not running.".into() })?,
        _ => cfg.base_url.clone(),
    };
    let model = cfg.model_id.clone().filter(|m| !m.is_empty()).ok_or_else(|| CoreError::Rejected { code: ReasonCode::ModelUnavailable, message: "No model selected. Pick one in Settings → Inference.".into() })?;
    let probe = ProviderConfig { base_url: base.clone(), ..cfg.clone() };
    if cfg.provider_kind != ProviderKind::ManagedLocal {
        authorize_endpoint(&probe).map_err(|e| CoreError::Rejected { code: ReasonCode::ModelUnavailable, message: e.to_string() })?;
    }
    Ok(Target {
        kind: cfg.provider_kind,
        is_loopback: is_loopback(&base),
        endpoint: Endpoint { base_url: base, api_key, timeout_secs: cfg.timeout_secs },
        model_id: model,
        temperature: cfg.temperature,
        reasoning_allowance: cfg.reasoning_allowance_tokens.clamp(0, 8000),
    })
}
