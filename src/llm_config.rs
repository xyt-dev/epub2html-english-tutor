//! Single source of truth for LLM provider configuration.
//!
//! Non-secret settings (API format, model, base URL, thinking toggle, ...)
//! live in one TOML file (default `llm.toml`). The API key itself is never
//! stored in that file: it is always read from an environment variable,
//! whose name is configurable via `api_key_env`.
//!
//! Precedence: CLI overrides > config file > built-in defaults.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use std::path::Path;

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
    fn parse(raw: &str) -> Result<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "anthropic" => Ok(Self::Anthropic),
            "openai" => Ok(Self::OpenAi),
            other => bail!(
                "unknown llm provider '{}': expected 'anthropic' or 'openai'",
                other
            ),
        }
    }

    fn default_base_url(self) -> &'static str {
        match self {
            Self::Anthropic => "https://api.anthropic.com",
            Self::OpenAi => "https://api.deepseek.com",
        }
    }

    fn default_api_key_env(self) -> &'static str {
        match self {
            Self::Anthropic => "ANTHROPIC_AUTH_TOKEN",
            Self::OpenAi => "DEEPSEEK_API_KEY",
        }
    }

    fn default_model(self) -> &'static str {
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

const DEFAULT_MAX_OUTPUT_TOKENS: u32 = 8192;
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 180;
const DEFAULT_THINKING_BUDGET_TOKENS: u32 = 4096;
const MIN_THINKING_BUDGET_TOKENS: u32 = 1024;
const DEFAULT_JOBS: usize = 2;

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
#[derive(Debug, Default, Deserialize)]
struct FileConfig {
    #[serde(default)]
    llm: LlmSection,
}

#[derive(Debug, Default, Deserialize)]
struct LlmSection {
    provider: Option<String>,
    model: Option<String>,
    base_url: Option<String>,
    thinking: Option<bool>,
    thinking_budget_tokens: Option<u32>,
    api_key_env: Option<String>,
    max_output_tokens: Option<u32>,
    request_timeout_secs: Option<u64>,
    jobs: Option<usize>,
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

/// Load `path` (if it exists), apply `overrides`, and resolve the API key
/// from the environment, producing a fully resolved [`LlmConfig`].
pub fn load_llm_config(path: &Path, overrides: &LlmConfigOverrides) -> Result<LlmConfig> {
    let mut section = if path.exists() {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read llm config '{}'", path.display()))?;
        toml::from_str::<FileConfig>(&raw)
            .with_context(|| format!("failed to parse llm config '{}'", path.display()))?
            .llm
    } else {
        LlmSection::default()
    };

    if let Some(provider) = &overrides.provider {
        section.provider = Some(provider.clone());
    }
    if let Some(model) = &overrides.model {
        section.model = Some(model.clone());
    }
    if let Some(base_url) = &overrides.base_url {
        section.base_url = Some(base_url.clone());
    }
    if let Some(thinking) = overrides.thinking {
        section.thinking = Some(thinking);
    }
    if let Some(jobs) = overrides.jobs {
        section.jobs = Some(jobs);
    }

    let format = section
        .provider
        .as_deref()
        .map(ApiFormat::parse)
        .transpose()?
        .unwrap_or(ApiFormat::Anthropic);

    let api_key_env = section
        .api_key_env
        .unwrap_or_else(|| format.default_api_key_env().to_string());
    let api_key = std::env::var(&api_key_env).with_context(|| {
        format!(
            "{} env var not set (configure a different variable via api_key_env in llm.toml)",
            api_key_env
        )
    })?;

    let max_output_tokens = section
        .max_output_tokens
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
    if max_output_tokens == 0 {
        bail!("llm config: max_output_tokens must be greater than 0");
    }

    let request_timeout_secs = section
        .request_timeout_secs
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    if request_timeout_secs == 0 {
        bail!("llm config: request_timeout_secs must be greater than 0");
    }

    let jobs = section.jobs.unwrap_or(DEFAULT_JOBS);
    if jobs == 0 {
        bail!("llm config: jobs must be at least 1");
    }

    let thinking = section.thinking.unwrap_or(false);
    let thinking_budget_tokens = section
        .thinking_budget_tokens
        .unwrap_or(DEFAULT_THINKING_BUDGET_TOKENS);

    if thinking && format == ApiFormat::Anthropic {
        if thinking_budget_tokens < MIN_THINKING_BUDGET_TOKENS {
            bail!(
                "llm config: thinking_budget_tokens must be at least {} when thinking is enabled",
                MIN_THINKING_BUDGET_TOKENS
            );
        }
        if thinking_budget_tokens >= max_output_tokens {
            bail!(
                "llm config: thinking_budget_tokens ({}) must be smaller than max_output_tokens ({})",
                thinking_budget_tokens,
                max_output_tokens
            );
        }
    }

    Ok(LlmConfig {
        format,
        model: section
            .model
            .unwrap_or_else(|| format.default_model().to_string()),
        base_url: section
            .base_url
            .unwrap_or_else(|| format.default_base_url().to_string()),
        thinking,
        thinking_budget_tokens,
        max_output_tokens,
        request_timeout_secs,
        api_key,
        jobs,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// Writes `contents` to a fresh temp file and returns its path together
    /// with a unique env var name to use for `api_key_env` in tests, so
    /// parallel tests never touch the same environment variable.
    fn temp_config(contents: &str) -> (std::path::PathBuf, String) {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("llm_config_test_{}_{}.toml", std::process::id(), n));
        std::fs::write(&path, contents).unwrap();
        let env_name = format!("LLM_CONFIG_TEST_KEY_{}_{}", std::process::id(), n);
        (path, env_name)
    }

    fn no_overrides() -> LlmConfigOverrides {
        LlmConfigOverrides::default()
    }

    #[test]
    fn defaults_to_anthropic_when_file_missing() {
        let env_name = "LLM_CONFIG_TEST_MISSING_FILE_KEY";
        std::env::set_var(env_name, "secret");
        let overrides = LlmConfigOverrides {
            provider: None,
            model: None,
            base_url: None,
            thinking: None,
            jobs: None,
        };
        // Point at a path that certainly doesn't exist.
        let path = std::env::temp_dir().join("llm_config_test_does_not_exist.toml");
        let _ = std::fs::remove_file(&path);

        // default_api_key_env for Anthropic is ANTHROPIC_AUTH_TOKEN, so this
        // will fail unless that's set; instead override provider explicitly
        // isn't possible without api_key_env, so just check the parse path
        // via an explicit anthropic provider override plus setting the real
        // default env var name via a scoped guard is avoided — assert on the
        // error message shape instead when the key env is absent.
        let result = load_llm_config(&path, &overrides);
        std::env::remove_var(env_name);
        // Either it fails because ANTHROPIC_AUTH_TOKEN isn't set in this
        // process, or (if it happens to be set) format resolves to Anthropic.
        if let Ok(cfg) = result {
            assert_eq!(cfg.format, ApiFormat::Anthropic);
            assert_eq!(cfg.base_url, "https://api.anthropic.com");
            assert!(!cfg.thinking);
        } else {
            let err = result.unwrap_err().to_string();
            assert!(err.contains("ANTHROPIC_AUTH_TOKEN"));
        }
    }

    #[test]
    fn reads_provider_model_and_thinking_from_file() {
        let (path, env_name) = temp_config("");
        // Rewrite with the generated env var name baked in.
        let contents = format!(
            "[llm]\nprovider = \"openai\"\nmodel = \"deepseek-v4-pro\"\nbase_url = \"https://api.deepseek.com\"\nthinking = true\napi_key_env = \"{}\"\n",
            env_name
        );
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-123");

        let cfg = load_llm_config(&path, &no_overrides()).unwrap();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert_eq!(cfg.format, ApiFormat::OpenAi);
        assert_eq!(cfg.model, "deepseek-v4-pro");
        assert_eq!(cfg.base_url, "https://api.deepseek.com");
        assert!(cfg.thinking);
        assert_eq!(cfg.api_key, "test-key-123");

        assert_eq!(cfg.jobs, DEFAULT_JOBS);
    }

    #[test]
    fn cli_overrides_take_precedence_over_file() {
        let (path, env_name) = temp_config("");
        let contents = format!(
            "[llm]\nprovider = \"anthropic\"\nmodel = \"file-model\"\nthinking = false\napi_key_env = \"{}\"\n",
            env_name
        );
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-456");

        let overrides = LlmConfigOverrides {
            provider: Some("openai".to_string()),
            model: Some("cli-model".to_string()),
            base_url: None,
            thinking: Some(true),
            jobs: Some(9),
        };
        let cfg = load_llm_config(&path, &overrides).unwrap();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert_eq!(cfg.format, ApiFormat::OpenAi);
        assert_eq!(cfg.model, "cli-model");
        assert!(cfg.thinking);
        // base_url falls back to the openai default since neither file nor
        // CLI specified it, but provider changed to openai via override.
        assert_eq!(cfg.base_url, "https://api.deepseek.com");

        assert_eq!(cfg.jobs, 9);
    }

    #[test]
    fn thinking_defaults_to_disabled() {
        let (path, env_name) = temp_config("");
        let contents = format!("[llm]\napi_key_env = \"{}\"\n", env_name);
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-789");

        let cfg = load_llm_config(&path, &no_overrides()).unwrap();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert!(!cfg.thinking);
    }

    #[test]
    fn rejects_undersized_thinking_budget_for_anthropic() {
        let (path, env_name) = temp_config("");
        let contents = format!(
            "[llm]\nprovider = \"anthropic\"\nthinking = true\nthinking_budget_tokens = 100\napi_key_env = \"{}\"\n",
            env_name
        );
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-000");

        let err = load_llm_config(&path, &no_overrides()).unwrap_err();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert!(err.to_string().contains("thinking_budget_tokens"));
    }

    #[test]
    fn rejects_unknown_provider() {
        let (path, env_name) = temp_config("");
        let contents = format!(
            "[llm]\nprovider = \"groq\"\napi_key_env = \"{}\"\n",
            env_name
        );
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-x");

        let err = load_llm_config(&path, &no_overrides()).unwrap_err();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert!(err.to_string().contains("unknown llm provider"));
    }

    #[test]
    fn reads_jobs_from_file() {
        let (path, env_name) = temp_config("");
        let contents = format!(
            "[llm]\nprovider = \"openai\"\njobs = 200\napi_key_env = \"{}\"\n",
            env_name
        );
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-jobs");

        let cfg = load_llm_config(&path, &no_overrides()).unwrap();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert_eq!(cfg.jobs, 200);
    }

    #[test]
    fn rejects_zero_jobs() {
        let (path, env_name) = temp_config("");
        let contents = format!(
            "[llm]\nprovider = \"openai\"\njobs = 0\napi_key_env = \"{}\"\n",
            env_name
        );
        std::fs::write(&path, contents).unwrap();
        std::env::set_var(&env_name, "test-key-zero-jobs");

        let err = load_llm_config(&path, &no_overrides()).unwrap_err();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert!(err.to_string().contains("jobs must be at least 1"));
    }
}
