//! On-disk and resolved shapes for LLM provider configuration.
//!
//! Non-secret settings (API format, model, base URL, thinking toggle, ...)
//! live in one TOML file. The API key itself is never stored in that file:
//! it is always read from an environment variable, whose name is
//! configurable via `api_key_env`.

use anyhow::{bail, Result};
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

impl std::fmt::Display for ApiFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Anthropic => "anthropic",
            Self::OpenAi => "openai",
        })
    }
}

pub const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 180;
pub const DEFAULT_THINKING_BUDGET_TOKENS: u32 = 4096;
pub const MIN_THINKING_BUDGET_TOKENS: u32 = 1024;
pub const DEFAULT_JOBS: usize = 2;

/// Fully resolved configuration ready to build an [`crate::llm_client::LlmClient`] from.
#[derive(Debug, Clone)]
pub struct LlmConfig {
    pub format: ApiFormat,
    pub model: String,
    pub base_url: String,
    pub thinking: bool,
    pub thinking_budget_tokens: u32,
    pub max_output_tokens: u32,
    pub request_timeout_secs: u64,
    pub api_key: String,
    pub jobs: usize,
}

/// Raw on-disk shape of the TOML file. Every field is optional so the file
/// itself is optional and each field can be partially specified.
#[derive(Debug, Default, Deserialize, Serialize)]
pub struct FileConfig {
    #[serde(default)]
    pub llm: LlmSection,
}

#[derive(Debug, Default, Deserialize, Serialize)]
pub struct LlmSection {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub thinking: Option<bool>,
    pub thinking_budget_tokens: Option<u32>,
    pub api_key_env: Option<String>,
    pub max_output_tokens: Option<u32>,
    pub request_timeout_secs: Option<u64>,
    pub jobs: Option<usize>,
}

/// CLI-provided overrides. `None` means "not specified, defer to the config
/// file / built-in default".
#[derive(Debug, Default, Clone)]
pub struct LlmConfigOverrides {
    pub provider: Option<String>,
    pub model: Option<String>,
    pub base_url: Option<String>,
    pub thinking: Option<bool>,
    pub jobs: Option<usize>,
}
