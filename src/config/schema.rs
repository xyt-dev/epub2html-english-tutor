//! On-disk profile templates and resolved LLM configuration.
//!
//! The platform TOML stores one or more reusable profile templates. A
//! template may contain an API key directly; `api_key_env` is consulted only
//! when that direct key is absent. Per-book state stores a resolved,
//! non-secret [`LlmProfileSnapshot`] so resumptions keep the same settings.

use anyhow::{bail, Result};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};

/// Wire format used to talk to the provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiFormat {
    /// Anthropic Messages API (`/v1/messages`). Also spoken by most
    /// Anthropic-compatible relay gateways (中转站).
    Anthropic,
    /// OpenAI Chat Completions API (`/chat/completions`). Also spoken by
    /// DeepSeek's native API.
    OpenAi,
}

impl ApiFormat {
    pub fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            other => bail!(
                "unknown llm provider '{}': expected 'anthropic' or 'openai'",
                other
            ),
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.deepseek.com",
        }
    }

    pub fn default_api_key_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_AUTH_TOKEN",
            Self::OpenAi => "DEEPSEEK_API_KEY",
        }
    }

    pub fn default_model(self) -> &'static str {
        "deepseek-v4-flash"
    }
}

pub(crate) fn is_deepseek_endpoint(base_url: &str) -> bool {
    reqwest::Url::parse(base_url)
        .ok()
        .and_then(|url| {
            url.host_str()
                .map(|host| host == "deepseek.com" || host.ends_with(".deepseek.com"))
        })
        .unwrap_or(false)
}

pub(crate) fn normalize_base_url(raw: &str) -> Result<String> {
    if raw.is_empty() || raw.trim() != raw {
        bail!("LLM base URL must be non-empty and have no surrounding whitespace");
    }
    let normalized = if raw.contains("://") {
        raw.to_string()
    } else {
        format!("https://{raw}")
    };
    let parsed = reqwest::Url::parse(&normalized)
        .map_err(|_| anyhow::anyhow!("invalid LLM base URL {:?}", raw))?;
    if !matches!(parsed.scheme(), "http" | "https") || parsed.host_str().is_none() {
        bail!("LLM base URL must use HTTP or HTTPS and include a host");
    }
    Ok(normalized.trim_end_matches('/').to_string())
}

impl std::fmt::Display for ApiFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        })
    }
}

/// Requested model thinking intensity.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ThinkingEffort {
    Low,
    Medium,
    #[default]
    High,
    #[serde(rename = "xhigh")]
    #[value(name = "xhigh", alias = "x-high")]
    XHigh,
    Max,
}

impl ThinkingEffort {
    pub fn normalized_deepseek(self) -> &'static str {
        match self {
            Self::Low | Self::Medium | Self::High => "high",
            Self::XHigh | Self::Max => "max",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::XHigh => "xhigh",
            Self::Max => "max",
        }
    }
}

impl std::fmt::Display for ThinkingEffort {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 32768;
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 180;
pub const DEFAULT_THINKING_EFFORT: ThinkingEffort = ThinkingEffort::High;
pub const DEFAULT_JOBS: usize = 20;

/// Fully resolved configuration ready to build an [`crate::llm_client::LlmClient`] from.
///
/// Deliberately does not implement `Debug`: `api_key` is secret.
#[derive(Clone)]
pub struct LlmConfig {
    pub profile_name: String,
    pub format: ApiFormat,
    pub model: String,
    pub base_url: String,
    pub thinking: bool,
    pub thinking_effort: ThinkingEffort,
    pub max_output_tokens: u32,
    pub request_timeout_secs: u64,
    pub api_key: String,
    pub jobs: usize,
}

/// The actual, non-secret configuration used by one resumable translation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LlmProfileSnapshot {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub base_url: String,
    pub thinking: bool,
    pub thinking_effort: ThinkingEffort,
    pub max_output_tokens: u32,
    pub request_timeout_secs: u64,
    /// Environment-variable locator used only when the current profile has no inline key.
    pub api_key_env: String,
    pub jobs: usize,
}

/// Platform configuration. Serializing `llm` produces a `[[llm]]` array of
/// tables. Deserialization also accepts the former singleton `[llm]` table.
#[derive(Default, Deserialize, Serialize)]
pub struct FileConfig {
    #[serde(default, deserialize_with = "deserialize_llm_profiles")]
    pub llm: Vec<LlmProfile>,
}

fn deserialize_llm_profiles<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<LlmProfile>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(LlmProfile),
        Many(Vec<LlmProfile>),
    }

    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(profile) => vec![profile],
        OneOrMany::Many(profiles) => profiles,
    })
}

/// Reusable model template stored under one `[[llm]]` entry.
///
/// Deliberately does not implement `Debug`: `api_key` may be present.
#[derive(Clone, Default, Deserialize, Serialize)]
pub struct LlmProfile {
    #[serde(default)]
    pub name: String,
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub thinking: Option<bool>,
    pub thinking_effort: Option<ThinkingEffort>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub api_key: Option<String>,
    pub api_key_env: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub request_timeout_secs: Option<u64>,
    pub jobs: Option<usize>,
}

/// CLI-provided overrides. `None` means "not specified, defer to the saved
/// effective configuration or selected profile template".
#[derive(Debug, Default, Clone)]
pub struct LlmConfigOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub thinking: Option<bool>,
    pub thinking_effort: Option<ThinkingEffort>,
    pub jobs: Option<usize>,
}

impl LlmConfigOverrides {
    pub fn is_empty(&self) -> bool {
        self.provider.is_none()
            && self.model.is_none()
            && self.base_url.is_none()
            && self.thinking.is_none()
            && self.thinking_effort.is_none()
            && self.jobs.is_none()
    }
}
