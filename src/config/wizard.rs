//! Interactive setup for appending reusable LLM profile templates.

use super::loader::{api_key_setup_command, read_file_config};
use super::schema::{
    normalize_base_url, ApiFormat, FileConfig, LlmProfile, ThinkingEffort, DEFAULT_JOBS,
    DEFAULT_MAX_OUTPUT_TOKENS,
};
use anyhow::{Context, Result};
use dialoguer::{Confirm, Input, Select};
use std::path::Path;

const PROVIDER_CHOICES: [&str; 2] = ["anthropic", "openai"];
const THINKING_EFFORT_CHOICES: [ThinkingEffort; 5] = [
    ThinkingEffort::Low,
    ThinkingEffort::Medium,
    ThinkingEffort::High,
    ThinkingEffort::XHigh,
    ThinkingEffort::Max,
];

/// Appends one profile template to the platform configuration.
pub fn run_setup_wizard(path: &Path) -> Result<()> {
    let mut config = read_file_config(path)?;

    println!("epub-reader LLM profile setup");
    println!("Config file: {}", path.display());
    println!("Existing profiles: {}", config.llm.len());
    println!();

    let provider_idx = Select::new()
        .with_prompt("LLM wire format / provider")
        .items(&PROVIDER_CHOICES)
        .default(1)
        .interact()?;
    let provider = PROVIDER_CHOICES[provider_idx];
    let format = ApiFormat::parse(provider)?;

    let model: String = Input::new()
        .with_prompt("Model name")
        .default(format.default_model().to_string())
        .validate_with(|value: &String| -> Result<(), &str> {
            if value.trim().is_empty() {
                Err("model must not be empty")
            } else if value.trim() != value {
                Err("model must not have surrounding whitespace")
            } else {
                Ok(())
            }
        })
        .interact_text()?;

    let existing_names = config
        .llm
        .iter()
        .map(|profile| profile.name.clone())
        .collect::<Vec<_>>();
    let profile_name: String = Input::new()
        .with_prompt("Profile name")
        .default(model.clone())
        .validate_with(move |value: &String| -> Result<(), &str> {
            if value.trim().is_empty() {
                Err("profile name must not be empty")
            } else if value.trim() != value {
                Err("profile name must not have surrounding whitespace")
            } else if existing_names
                .iter()
                .any(|name| name.eq_ignore_ascii_case(value))
            {
                Err("a profile with this name already exists")
            } else {
                Ok(())
            }
        })
        .interact_text()?;

    let base_url: String = Input::new()
        .with_prompt("API base URL")
        .default(format.default_base_url().to_string())
        .validate_with(|value: &String| -> Result<(), &str> {
            if value.trim().is_empty() {
                Err("base URL must not be empty")
            } else if value.trim() != value {
                Err("base URL must not have surrounding whitespace")
            } else {
                Ok(())
            }
        })
        .interact_text()?;
    let base_url = normalize_base_url(&base_url)?;

    let entered_api_key: String = Input::new()
        .with_prompt("API key (visible; leave blank to use an environment variable fallback)")
        .allow_empty(true)
        .validate_with(|value: &String| -> Result<(), &str> {
            if !value.is_empty() && value.trim().len() != value.len() {
                Err("API key must not have surrounding whitespace")
            } else {
                Ok(())
            }
        })
        .interact_text()?;
    let api_key = (!entered_api_key.is_empty()).then_some(entered_api_key);
    let api_key_env = if api_key.is_none() {
        Some(
            Input::new()
                .with_prompt("Fallback environment variable holding the API key")
                .default(format.default_api_key_env().to_string())
                .validate_with(|value: &String| -> Result<(), &str> {
                    let mut bytes = value.bytes();
                    if !matches!(bytes.next(), Some(b'A'..=b'Z' | b'a'..=b'z' | b'_'))
                        || !bytes.all(|byte| {
                            matches!(
                                byte,
                                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'_'
                            )
                        })
                    {
                        Err("enter an environment variable name, not an API key")
                    } else {
                        Ok(())
                    }
                })
                .interact_text()?,
        )
    } else {
        None
    };

    let thinking = Confirm::new()
        .with_prompt("Enable model thinking/reasoning mode?")
        .default(false)
        .interact()?;
    let thinking_effort = if thinking {
        let effort_idx = Select::new()
            .with_prompt("Thinking/reasoning effort")
            .items(&THINKING_EFFORT_CHOICES)
            .default(2)
            .interact()?;
        Some(THINKING_EFFORT_CHOICES[effort_idx])
    } else {
        None
    };

    let max_output_tokens: u32 = Input::new()
        .with_prompt("Maximum output tokens per request")
        .default(DEFAULT_MAX_OUTPUT_TOKENS)
        .validate_with(|value: &u32| -> Result<(), &str> {
            if *value >= 1 {
                Ok(())
            } else {
                Err("maximum output tokens must be at least 1")
            }
        })
        .interact_text()?;

    let jobs: usize = Input::new()
        .with_prompt("Concurrent translation requests (jobs)")
        .default(DEFAULT_JOBS)
        .validate_with(|value: &usize| -> Result<(), &str> {
            if *value >= 1 {
                Ok(())
            } else {
                Err("jobs must be at least 1")
            }
        })
        .interact_text()?;

    let profile = LlmProfile {
        name: profile_name.clone(),
        provider: Some(provider.to_string()),
        model: Some(model),
        base_url: Some(base_url),
        thinking: Some(thinking),
        thinking_effort,
        api_key,
        api_key_env: api_key_env.clone(),
        max_output_tokens: Some(max_output_tokens),
        request_timeout_secs: None,
        jobs: Some(jobs),
    };
    append_profile_and_save(path, &mut config, profile)?;

    println!();
    println!("Added profile {:?} to {}", profile_name, path.display());
    if let Some(api_key_env) = api_key_env {
        println!(
            "Set its fallback API key before translating:\n  {}",
            api_key_setup_command(&api_key_env)
        );
    } else {
        println!("The API key is stored in the owner-only platform config file.");
    }
    Ok(())
}

fn append_profile_and_save(
    path: &Path,
    config: &mut FileConfig,
    profile: LlmProfile,
) -> Result<()> {
    config.llm.push(profile);
    let rendered =
        toml::to_string_pretty(config).context("failed to serialize LLM profile config")?;
    crate::fs_utils::atomic_write_private(path, rendered.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_config_path() -> std::path::PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "epub_reader_wizard_{}_{}.toml",
            std::process::id(),
            n
        ))
    }

    fn profile(name: &str, api_key: &str) -> LlmProfile {
        LlmProfile {
            name: name.to_string(),
            provider: Some("openai".to_string()),
            model: Some(format!("{name}-model")),
            base_url: Some("https://api.deepseek.com".to_string()),
            thinking: Some(false),
            thinking_effort: None,
            api_key: Some(api_key.to_string()),
            api_key_env: None,
            max_output_tokens: None,
            request_timeout_secs: None,
            jobs: Some(2),
        }
    }

    #[test]
    fn setup_save_appends_without_changing_existing_profile() {
        let path = temp_config_path();
        let mut config = FileConfig {
            llm: vec![profile("existing", "existing-key")],
        };

        append_profile_and_save(&path, &mut config, profile("added", "added-key")).unwrap();
        let loaded = read_file_config(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(loaded.llm.len(), 2);
        assert_eq!(loaded.llm[0].name, "existing");
        assert_eq!(loaded.llm[0].api_key.as_deref(), Some("existing-key"));
        assert_eq!(loaded.llm[1].name, "added");
        assert_eq!(loaded.llm[1].api_key.as_deref(), Some("added-key"));
    }

    #[test]
    fn setup_save_migrates_legacy_single_table_to_profile_list() {
        let path = temp_config_path();
        std::fs::write(
            &path,
            "[llm]\nprovider = \"openai\"\nmodel = \"legacy-model\"\napi_key = \"legacy-key\"\n",
        )
        .unwrap();
        let mut config = read_file_config(&path).unwrap();

        append_profile_and_save(&path, &mut config, profile("new", "new-key")).unwrap();
        let rendered = std::fs::read_to_string(&path).unwrap();
        let loaded = read_file_config(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert_eq!(rendered.matches("[[llm]]").count(), 2);
        assert!(!rendered.lines().any(|line| line == "[llm]"));
        assert_eq!(loaded.llm.len(), 2);
        assert_eq!(loaded.llm[0].name, "default");
        assert_eq!(loaded.llm[1].name, "new");
    }
}
