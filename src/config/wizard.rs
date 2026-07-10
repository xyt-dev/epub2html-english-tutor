//! Interactive terminal setup wizard for LLM provider configuration.
//!
//! Prompts for the handful of settings people actually tweak (provider,
//! model, base URL, API key env var name, thinking, concurrency) and writes
//! the result to the user's platform config file. Advanced fields already
//! present in that file (`max_output_tokens`, `request_timeout_secs`,
//! `thinking_budget_tokens`) are preserved as-is; edit them by hand if
//! needed.

use super::schema::{ApiFormat, FileConfig, LlmSection, DEFAULT_JOBS};
use anyhow::{Context, Result};
use dialoguer::{Confirm, Input, Select};
use std::path::Path;

const PROVIDER_CHOICES: [&str; 2] = ["anthropic", "openai"];

fn existing_section(path: &Path) -> LlmSection {
    if !path.exists() {
        return LlmSection::default();
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| toml::from_str::<FileConfig>(&raw).ok())
        .map(|cfg| cfg.llm)
        .unwrap_or_default()
}

/// Runs the interactive wizard and writes the resulting config to `path`.
pub fn run_setup_wizard(path: &Path) -> Result<()> {
    let mut section = existing_section(path);

    println!("epub-reader LLM setup");
    println!("Config file: {}", path.display());
    println!();

    let default_provider_idx = section
        .provider
        .as_deref()
        .and_then(|p| PROVIDER_CHOICES.iter().position(|c| *c == p))
        .unwrap_or(1);
    let provider_idx = Select::new()
        .with_prompt("LLM wire format / provider")
        .items(&PROVIDER_CHOICES)
        .default(default_provider_idx)
        .interact()?;
    let provider = PROVIDER_CHOICES[provider_idx];
    let format = ApiFormat::parse(provider)?;
    section.provider = Some(provider.to_string());

    let model: String = Input::new()
        .with_prompt("Model name")
        .default(
            section
                .model
                .clone()
                .unwrap_or_else(|| format.default_model().to_string()),
        )
        .interact_text()?;
    section.model = Some(model);

    let base_url: String = Input::new()
        .with_prompt("API base URL")
        .default(
            section
                .base_url
                .clone()
                .unwrap_or_else(|| format.default_base_url().to_string()),
        )
        .interact_text()?;
    section.base_url = Some(base_url);

    let api_key_env: String = Input::new()
        .with_prompt("Environment variable holding the API key")
        .default(
            section
                .api_key_env
                .clone()
                .unwrap_or_else(|| format.default_api_key_env().to_string()),
        )
        .interact_text()?;
    section.api_key_env = Some(api_key_env.clone());

    let thinking = Confirm::new()
        .with_prompt("Enable model thinking/reasoning mode?")
        .default(section.thinking.unwrap_or(false))
        .interact()?;
    section.thinking = Some(thinking);

    let jobs: usize = Input::new()
        .with_prompt("Concurrent translation requests (jobs)")
        .default(section.jobs.unwrap_or(DEFAULT_JOBS))
        .validate_with(|value: &usize| -> Result<(), &str> {
            if *value >= 1 {
                Ok(())
            } else {
                Err("jobs must be at least 1")
            }
        })
        .interact_text()?;
    section.jobs = Some(jobs);

    let rendered = toml::to_string_pretty(&FileConfig { llm: section })
        .context("failed to serialize llm config")?;
    crate::fs_utils::atomic_write(path, rendered.as_bytes())?;

    println!();
    println!("Saved config to {}", path.display());
    println!(
        "Set the API key before translating: export {}=\"...\"",
        api_key_env
    );

    Ok(())
}
