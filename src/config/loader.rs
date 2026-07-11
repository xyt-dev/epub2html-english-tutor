//! Loads, validates, snapshots, and resolves reusable LLM profile templates.
//!
//! A new binding materializes CLI overrides over one template. A resume uses
//! the saved non-secret snapshot unchanged. Credentials are resolved only
//! after that snapshot has been persisted.

use super::schema::{
    normalize_base_url, ApiFormat, FileConfig, LlmConfig, LlmConfigOverrides, LlmProfile,
    LlmProfileSnapshot, DEFAULT_JOBS, DEFAULT_MAX_OUTPUT_TOKENS, DEFAULT_REQUEST_TIMEOUT_SECS,
    DEFAULT_THINKING_EFFORT,
};
use anyhow::{anyhow, bail, Context, Result};
use std::collections::HashSet;
use std::path::Path;

pub(super) fn read_file_config(path: &Path) -> Result<FileConfig> {
    let (mut config, legacy_singleton) = if path.exists() {
        let raw = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read LLM config '{}'", path.display()))?;
        let document = toml::from_str::<toml::Value>(&raw).map_err(|_| {
            anyhow!(
                "failed to parse LLM config '{}'; fix its TOML syntax",
                path.display()
            )
        })?;
        let legacy_singleton = matches!(document.get("llm"), Some(toml::Value::Table(_)));
        let config = toml::from_str::<FileConfig>(&raw).map_err(|_| {
            anyhow!(
                "failed to parse LLM config '{}'; check its profile fields",
                path.display()
            )
        })?;
        (config, legacy_singleton)
    } else {
        (FileConfig::default(), false)
    };
    normalize_profiles(path, &mut config.llm, legacy_singleton)?;
    Ok(config)
}

fn normalize_profiles(
    path: &Path,
    profiles: &mut [LlmProfile],
    legacy_singleton: bool,
) -> Result<()> {
    let mut names = HashSet::with_capacity(profiles.len());

    for (index, profile) in profiles.iter_mut().enumerate() {
        if profile.name.trim().is_empty() {
            if legacy_singleton {
                profile.name = "default".to_string();
            } else {
                bail!(
                    "LLM config '{}': profile #{} needs a non-empty `name`",
                    path.display(),
                    index + 1
                );
            }
        } else if profile.name.trim() != profile.name || profile.name.chars().any(char::is_control)
        {
            bail!(
                "LLM config '{}': profile name {:?} is invalid",
                path.display(),
                profile.name
            );
        }

        if !names.insert(profile.name.to_ascii_lowercase()) {
            bail!(
                "LLM config '{}': duplicate profile name {:?}",
                path.display(),
                profile.name
            );
        }

        // One-time compatibility repair for the legacy singleton format:
        // users sometimes put the secret itself in `api_key_env`.
        if legacy_singleton
            && profile
                .api_key_env
                .as_deref()
                .is_some_and(looks_like_api_key)
        {
            if profile.api_key.is_some() {
                bail!(
                    "LLM config '{}': legacy profile {:?} contains credentials in both \
                     `api_key` and `api_key_env`; values were hidden",
                    path.display(),
                    profile.name
                );
            }
            profile.api_key = profile.api_key_env.take();
        }

        validate_secret_template(path, profile)?;
        materialize_llm_profile(profile, &LlmConfigOverrides::default())?;
    }
    Ok(())
}

fn looks_like_api_key(value: &str) -> bool {
    let value = value.trim();
    value.len() >= 16
        && value.get(..3).is_some_and(|prefix| {
            prefix.eq_ignore_ascii_case("sk-") || prefix.eq_ignore_ascii_case("sk_")
        })
}

fn validate_secret_template(path: &Path, profile: &LlmProfile) -> Result<()> {
    if let Some(api_key) = profile.api_key.as_deref() {
        if api_key.trim().is_empty() || api_key.trim().len() != api_key.len() {
            bail!(
                "LLM config '{}': profile {:?} has an empty or whitespace-padded `api_key`; \
                 re-enter the key",
                path.display(),
                profile.name
            );
        }
        return Ok(());
    }

    if let Some(api_key_env) = profile.api_key_env.as_deref() {
        if looks_like_api_key(api_key_env) || !is_valid_env_name(api_key_env) {
            bail!(
                "LLM config '{}': profile {:?} has an invalid `api_key_env`. It must contain \
                 an environment variable name, not an API key; the configured value was hidden. \
                 Move a direct key to `api_key` or use a name such as {:?}.",
                path.display(),
                profile.name,
                profile_format(profile)?.default_api_key_env()
            );
        }
    }
    Ok(())
}

fn is_valid_env_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
        && bytes.all(|byte| matches!(byte, b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'))
}

fn profile_format(profile: &LlmProfile) -> Result<ApiFormat> {
    profile
        .provider
        .as_deref()
        .map(ApiFormat::parse)
        .transpose()
        .map(|format| format.unwrap_or(ApiFormat::Anthropic))
}

pub(super) fn api_key_setup_command(api_key_env: &str) -> String {
    if cfg!(windows) {
        format!("$env:{api_key_env} = \"...\"")
    } else {
        format!("export {api_key_env}=\"...\"")
    }
}

fn missing_api_key_error(path: &Path, profile: &LlmProfile, api_key_env: &str) -> anyhow::Error {
    anyhow!(
        "API key is not configured for profile {:?}.\n\
         Add `api_key = \"...\"` to that profile's `[[llm]]` entry in '{}', or set the fallback \
         environment variable {api_key_env}:\n  {}",
        profile.name,
        path.display(),
        api_key_setup_command(api_key_env)
    )
}

fn resolve_api_key(path: &Path, profile: &LlmProfile, api_key_env: &str) -> Result<String> {
    if let Some(api_key) = profile.api_key.as_ref() {
        return Ok(api_key.clone());
    }

    let api_key = match std::env::var(api_key_env) {
        Ok(api_key) => api_key,
        Err(std::env::VarError::NotPresent) => {
            return Err(missing_api_key_error(path, profile, api_key_env));
        }
        Err(std::env::VarError::NotUnicode(_)) => {
            bail!(
                "API key environment variable {api_key_env} for profile {:?} contains \
                 non-Unicode data; reset it:\n  {}",
                profile.name,
                api_key_setup_command(api_key_env)
            );
        }
    };

    let trimmed = api_key.trim();
    if trimmed.is_empty() {
        return Err(missing_api_key_error(path, profile, api_key_env));
    }
    if trimmed.len() != api_key.len() {
        bail!(
            "API key environment variable {api_key_env} for profile {:?} has leading or \
             trailing whitespace; reset it:\n  {}",
            profile.name,
            api_key_setup_command(api_key_env)
        );
    }
    Ok(api_key)
}

fn validate_snapshot(snapshot: &LlmProfileSnapshot) -> Result<ApiFormat> {
    let format = ApiFormat::parse(&snapshot.provider)?;
    if snapshot.name.trim().is_empty() || snapshot.name.chars().any(char::is_control) {
        bail!("saved LLM profile has an invalid name");
    }
    if snapshot.model.trim().is_empty() {
        bail!("saved LLM profile {:?} has an empty model", snapshot.name);
    }
    normalize_base_url(&snapshot.base_url).map_err(|error| {
        anyhow!(
            "saved LLM profile {:?} has an invalid base URL: {}",
            snapshot.name,
            error
        )
    })?;
    if snapshot.max_output_tokens == 0 {
        bail!(
            "saved LLM profile {:?}: max_output_tokens must be greater than 0",
            snapshot.name
        );
    }
    if snapshot.request_timeout_secs == 0 {
        bail!(
            "saved LLM profile {:?}: request_timeout_secs must be greater than 0",
            snapshot.name
        );
    }
    if snapshot.jobs == 0 {
        bail!(
            "saved LLM profile {:?}: jobs must be at least 1",
            snapshot.name
        );
    }
    if !is_valid_env_name(&snapshot.api_key_env) || looks_like_api_key(&snapshot.api_key_env) {
        bail!(
            "saved LLM profile {:?} has an invalid API key environment locator",
            snapshot.name
        );
    }
    Ok(format)
}

/// Materializes one reusable template into the actual non-secret settings
/// persisted at the head of a book's state JSON. No environment is read.
pub fn materialize_llm_profile(
    profile: &LlmProfile,
    overrides: &LlmConfigOverrides,
) -> Result<LlmProfileSnapshot> {
    let provider = overrides
        .provider
        .as_deref()
        .or(profile.provider.as_deref());
    let format = provider
        .map(ApiFormat::parse)
        .transpose()?
        .unwrap_or(ApiFormat::Anthropic);
    let model = overrides
        .model
        .clone()
        .or_else(|| profile.model.clone())
        .unwrap_or_else(|| format.default_model().to_string());
    let base_url = normalize_base_url(
        overrides
            .base_url
            .as_deref()
            .or(profile.base_url.as_deref())
            .unwrap_or_else(|| format.default_base_url()),
    )?;
    let thinking = overrides.thinking.or(profile.thinking).unwrap_or(false);
    let thinking_effort = overrides
        .thinking_effort
        .or(profile.thinking_effort)
        .unwrap_or(DEFAULT_THINKING_EFFORT);
    let max_output_tokens = profile
        .max_output_tokens
        .unwrap_or(DEFAULT_MAX_OUTPUT_TOKENS);
    let request_timeout_secs = profile
        .request_timeout_secs
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT_SECS);
    let jobs = overrides.jobs.or(profile.jobs).unwrap_or(DEFAULT_JOBS);
    let api_key_env = profile
        .api_key_env
        .clone()
        .unwrap_or_else(|| format.default_api_key_env().to_string());

    let snapshot = LlmProfileSnapshot {
        name: profile.name.clone(),
        provider: format.to_string(),
        model,
        base_url,
        thinking,
        thinking_effort,
        max_output_tokens,
        request_timeout_secs,
        api_key_env,
        jobs,
    };
    validate_snapshot(&snapshot)?;
    Ok(snapshot)
}

/// Loads and validates the reusable template list without reading any key.
pub fn load_llm_profiles(path: &Path) -> Result<Vec<LlmProfile>> {
    let config = read_file_config(path)?;
    if config.llm.is_empty() {
        bail!(
            "no LLM profiles configured in '{}'; run `epub-reader --setup` to add one",
            path.display()
        );
    }
    Ok(config.llm)
}

/// Resolves the rotatable credential for a previously materialized snapshot.
/// All non-secret runtime settings come exclusively from that snapshot.
pub fn resolve_llm_profile(
    path: &Path,
    profile: &LlmProfile,
    snapshot: &LlmProfileSnapshot,
) -> Result<LlmConfig> {
    validate_secret_template(path, profile)?;
    if snapshot.name != profile.name {
        bail!(
            "saved profile {:?} does not match configured profile {:?}",
            snapshot.name,
            profile.name
        );
    }
    let format = validate_snapshot(snapshot)?;
    let api_key = resolve_api_key(path, profile, &snapshot.api_key_env)?;
    Ok(LlmConfig {
        profile_name: snapshot.name.clone(),
        format,
        model: snapshot.model.clone(),
        base_url: normalize_base_url(&snapshot.base_url)?,
        thinking: snapshot.thinking,
        thinking_effort: snapshot.thinking_effort,
        max_output_tokens: snapshot.max_output_tokens,
        request_timeout_secs: snapshot.request_timeout_secs,
        api_key,
        jobs: snapshot.jobs,
    })
}

#[cfg(test)]
mod tests {
    use super::super::schema::ThinkingEffort;
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_config(contents: &str) -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("llm_profiles_{}_{}.toml", std::process::id(), n));
        std::fs::write(&path, contents).unwrap();
        path
    }

    fn unique_env() -> String {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        format!("LLM_PROFILE_TEST_KEY_{}_{}", std::process::id(), n)
    }

    fn profile(name: &str) -> LlmProfile {
        LlmProfile {
            name: name.to_string(),
            provider: Some("openai".to_string()),
            model: Some("test-model".to_string()),
            base_url: Some("https://api.deepseek.com".to_string()),
            thinking: Some(false),
            thinking_effort: Some(ThinkingEffort::High),
            api_key: Some("inline-test-key".to_string()),
            api_key_env: None,
            max_output_tokens: None,
            request_timeout_secs: None,
            jobs: Some(2),
        }
    }

    #[test]
    fn loads_canonical_profile_list_without_reading_credentials() {
        let path = temp_config(
            "[[llm]]\nname = \"first\"\nprovider = \"openai\"\napi_key_env = \"MISSING_FIRST_KEY\"\n\
             \n[[llm]]\nname = \"second\"\nprovider = \"anthropic\"\napi_key_env = \"MISSING_SECOND_KEY\"\n",
        );

        let profiles = load_llm_profiles(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(profiles.len(), 2);
        assert_eq!(profiles[0].name, "first");
        assert_eq!(profiles[1].name, "second");
    }

    #[test]
    fn loads_legacy_singleton_as_default_profile() {
        let path = temp_config(
            "[llm]\nprovider = \"openai\"\nmodel = \"legacy-model\"\napi_key_env = \"LEGACY_KEY\"\n",
        );

        let profiles = load_llm_profiles(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(profiles.len(), 1);
        assert_eq!(profiles[0].name, "default");
        assert_eq!(profiles[0].model.as_deref(), Some("legacy-model"));
    }

    #[test]
    fn migrates_legacy_key_misplaced_in_env_field_in_memory() {
        let secret = "sk-test-sentinel-legacy-123456";
        let path = temp_config(&format!(
            "[llm]\nprovider = \"openai\"\napi_key_env = \"{secret}\"\n"
        ));

        let config = read_file_config(&path).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(config.llm[0].api_key.as_deref(), Some(secret));
        assert!(config.llm[0].api_key_env.is_none());
        assert!(raw.contains("api_key_env"));
    }

    #[test]
    fn canonical_key_in_env_field_is_rejected_without_echoing_it() {
        let secret = "sk-test-sentinel-canonical-123456";
        let path = temp_config(&format!(
            "[[llm]]\nname = \"bad\"\nprovider = \"openai\"\napi_key_env = \"{secret}\"\n"
        ));

        let error = load_llm_profiles(&path)
            .err()
            .expect("key-like environment locator must fail");
        let _ = std::fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("configured value was hidden"), "{message}");
        assert!(!message.contains(secret), "{message}");
    }

    #[test]
    fn rejects_nameless_and_duplicate_canonical_profiles() {
        let nameless = temp_config("[[llm]]\nprovider = \"openai\"\n");
        let error = load_llm_profiles(&nameless)
            .err()
            .expect("canonical profile must have a name");
        let _ = std::fs::remove_file(&nameless);
        assert!(error.to_string().contains("non-empty `name`"));

        let duplicate = temp_config(
            "[[llm]]\nname = \"same\"\nprovider = \"openai\"\n\
             \n[[llm]]\nname = \"SAME\"\nprovider = \"anthropic\"\n",
        );
        let error = load_llm_profiles(&duplicate)
            .err()
            .expect("duplicate profile names must fail");
        let _ = std::fs::remove_file(&duplicate);
        assert!(error.to_string().contains("duplicate profile name"));
    }

    #[test]
    fn serialization_always_emits_profile_array() {
        let config = FileConfig {
            llm: vec![profile("one"), profile("two")],
        };

        let rendered = toml::to_string_pretty(&config).unwrap();

        assert_eq!(rendered.matches("[[llm]]").count(), 2);
        assert!(!rendered.lines().any(|line| line == "[llm]"));
    }

    #[test]
    fn materializes_defaults_and_cli_overrides() {
        let mut template = LlmProfile {
            name: "configured".to_string(),
            ..LlmProfile::default()
        };
        template.api_key = Some("inline-test-key".to_string());
        let overrides = LlmConfigOverrides {
            provider: Some("openai".to_string()),
            model: Some("cli-model".to_string()),
            base_url: None,
            thinking: Some(true),
            thinking_effort: Some(ThinkingEffort::Max),
            jobs: Some(9),
        };

        let snapshot = materialize_llm_profile(&template, &overrides).unwrap();

        assert_eq!(snapshot.name, "configured");
        assert_eq!(snapshot.provider, "openai");
        assert_eq!(snapshot.model, "cli-model");
        assert_eq!(snapshot.base_url, "https://api.deepseek.com");
        assert!(snapshot.thinking);
        assert_eq!(snapshot.thinking_effort, ThinkingEffort::Max);
        assert_eq!(snapshot.api_key_env, "DEEPSEEK_API_KEY");
        assert_eq!(snapshot.jobs, 9);
    }

    #[test]
    fn accepts_every_thinking_effort_value() {
        for effort in [
            ThinkingEffort::Low,
            ThinkingEffort::Medium,
            ThinkingEffort::High,
            ThinkingEffort::XHigh,
            ThinkingEffort::Max,
        ] {
            let mut template = profile("effort");
            template.thinking = Some(true);
            template.thinking_effort = Some(effort);

            let snapshot =
                materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();
            assert_eq!(snapshot.thinking_effort, effort);
        }
    }

    #[test]
    fn validates_each_numeric_profile_setting() {
        type NumericCase = (&'static str, fn(&mut LlmProfile), &'static str);
        let cases: [NumericCase; 3] = [
            (
                "max output",
                |profile| profile.max_output_tokens = Some(0),
                "max_output_tokens",
            ),
            (
                "timeout",
                |profile| profile.request_timeout_secs = Some(0),
                "request_timeout_secs",
            ),
            ("jobs", |profile| profile.jobs = Some(0), "jobs"),
        ];

        for (name, mutate, expected) in cases {
            let mut template = profile(name);
            mutate(&mut template);
            let error = match materialize_llm_profile(&template, &LlmConfigOverrides::default()) {
                Ok(_) => panic!("invalid numeric setting must fail"),
                Err(error) => error,
            };
            assert!(error.to_string().contains(expected), "{name}: {error}");
        }
    }

    #[test]
    fn uses_higher_default_and_honors_profile_output_limit() {
        let mut template = profile("tokens");
        template.max_output_tokens = None;
        let default_snapshot =
            materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();

        template.max_output_tokens = Some(65536);
        let configured_snapshot =
            materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();

        assert_eq!(default_snapshot.max_output_tokens, 32768);
        assert_eq!(configured_snapshot.max_output_tokens, 65536);
    }

    #[test]
    fn normalizes_scheme_less_base_url_to_https() {
        let mut template = profile("url");
        template.base_url = Some("apiclaude.cc".to_string());

        let snapshot = materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();

        assert_eq!(snapshot.base_url, "https://apiclaude.cc");
    }

    #[test]
    fn rejects_non_http_base_url_scheme() {
        let mut template = profile("url");
        template.base_url = Some("ftp://example.com".to_string());

        let error = materialize_llm_profile(&template, &LlmConfigOverrides::default())
            .expect_err("non-HTTP base URL must fail");

        assert!(error.to_string().contains("HTTP or HTTPS"));
    }

    #[test]
    fn inline_key_wins_over_environment_fallback() {
        let env_name = unique_env();
        std::env::set_var(&env_name, "environment-key");
        let mut template = profile("inline");
        template.api_key_env = Some(env_name.clone());
        let snapshot = materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();

        let config = resolve_llm_profile(Path::new("config.toml"), &template, &snapshot).unwrap();
        std::env::remove_var(&env_name);

        assert_eq!(config.api_key, "inline-test-key");
    }

    #[test]
    fn environment_key_is_read_only_for_selected_resolution() {
        let env_name = unique_env();
        std::env::remove_var(&env_name);
        let path = temp_config(&format!(
            "[[llm]]\nname = \"env\"\nprovider = \"openai\"\napi_key_env = \"{env_name}\"\n"
        ));
        let profiles = load_llm_profiles(&path).unwrap();
        let snapshot =
            materialize_llm_profile(&profiles[0], &LlmConfigOverrides::default()).unwrap();

        std::env::set_var(&env_name, "environment-key");
        let config = resolve_llm_profile(&path, &profiles[0], &snapshot).unwrap();
        std::env::remove_var(&env_name);
        let _ = std::fs::remove_file(&path);

        assert_eq!(config.api_key, "environment-key");
    }

    #[test]
    fn missing_blank_and_padded_environment_keys_fail_locally() {
        for value in [None, Some(""), Some(" \t "), Some(" padded-key ")] {
            let env_name = unique_env();
            match value {
                Some(value) => std::env::set_var(&env_name, value),
                None => std::env::remove_var(&env_name),
            }
            let mut template = profile("fallback");
            template.api_key = None;
            template.api_key_env = Some(env_name.clone());
            let snapshot =
                materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();

            let error = resolve_llm_profile(Path::new("config.toml"), &template, &snapshot)
                .err()
                .expect("invalid fallback key must fail before HTTP");
            std::env::remove_var(&env_name);
            let message = error.to_string();
            assert!(message.contains(&env_name), "{message}");
            assert!(
                message.contains("not configured") || message.contains("whitespace"),
                "{message}"
            );
        }
    }

    #[test]
    fn invalid_inline_key_never_falls_back_to_environment() {
        let env_name = unique_env();
        std::env::set_var(&env_name, "valid-environment-key");
        let mut template = profile("invalid-inline");
        template.api_key = Some(" padded-inline-key ".to_string());
        template.api_key_env = Some(env_name.clone());
        let snapshot = materialize_llm_profile(&template, &LlmConfigOverrides::default()).unwrap();

        let error = resolve_llm_profile(Path::new("config.toml"), &template, &snapshot)
            .err()
            .expect("invalid inline key must not fall back");
        std::env::remove_var(&env_name);

        assert!(error.to_string().contains("whitespace-padded `api_key`"));
    }

    #[test]
    fn saved_snapshot_is_authoritative_while_inline_key_rotates() {
        let original = profile("resume");
        let snapshot = materialize_llm_profile(&original, &LlmConfigOverrides::default()).unwrap();
        let mut changed = original.clone();
        changed.provider = Some("anthropic".to_string());
        changed.model = Some("changed-model".to_string());
        changed.base_url = Some("https://changed.example".to_string());
        changed.jobs = Some(99);
        changed.api_key = Some("rotated-key".to_string());

        let config = resolve_llm_profile(Path::new("config.toml"), &changed, &snapshot).unwrap();

        assert_eq!(config.format, ApiFormat::OpenAi);
        assert_eq!(config.model, "test-model");
        assert_eq!(config.base_url, "https://api.deepseek.com");
        assert_eq!(config.jobs, 2);
        assert_eq!(config.api_key, "rotated-key");
    }

    #[test]
    fn parse_errors_never_echo_inline_secrets() {
        let secret = "sk-test-sentinel-parse-error-123456";
        let path = temp_config(&format!(
            "[[llm]]\nname = \"broken\"\napi_key = \"{secret}\"\ninvalid = [\n"
        ));

        let error = load_llm_profiles(&path)
            .err()
            .expect("malformed TOML must fail");
        let _ = std::fs::remove_file(&path);
        let message = error.to_string();

        assert!(message.contains("failed to parse LLM config"), "{message}");
        assert!(!message.contains(secret), "{message}");
    }

    #[test]
    fn missing_catalog_directs_user_to_setup() {
        let path = temp_config("");
        std::fs::remove_file(&path).unwrap();

        let error = load_llm_profiles(&path)
            .err()
            .expect("missing catalog must require setup");

        assert!(error.to_string().contains("epub-reader --setup"));
    }
}
