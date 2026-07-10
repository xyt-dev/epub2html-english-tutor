//! Loads, merges, and validates LLM configuration.
//!
//! Precedence: CLI overrides > user config file > built-in defaults.
//!
//! The user config file normally lives in the platform config directory
//! (see [`super::paths`]). The project's bundled `llm.toml` is only a
//! human-readable template/example; it is never read by the running
//! program. When no user config file exists, callers fall back to CLI
//! overrides plus built-in defaults (see [`run_setup_wizard`] to create one
//! interactively).

use super::schema::{
    ApiFormat, FileConfig, LlmConfig, LlmConfigOverrides, LlmSection, DEFAULT_JOBS,
    DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_REQUEST_TIMEOUT_SECS, DEFAULT_THINKING_BUDGET_TOKENS,
    MIN_THINKING_BUDGET_TOKENS,
};
use anyhow::{bail, Context, Result};
use std::path::Path;

fn read_section(path: &Path) -> Result<LlmSection> {
    if !path.exists() {
        return Ok(LlmSection::default());
    }
    let raw = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read llm config '{}'", path.display()))?;
    Ok(toml::from_str::<FileConfig>(&raw)
        .with_context(|| format!("failed to parse llm config '{}'", path.display()))?
        .llm)
}

/// Load `path` (if it exists), apply `overrides`, and resolve the API key
/// from the environment, producing a fully resolved [`LlmConfig`].
pub fn load_llm_config(path: &Path, overrides: &LlmConfigOverrides) -> Result<LlmConfig> {
    let mut section = read_section(path)?;

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
            "{} env var not set (configure a different variable via api_key_env in the llm config)",
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
