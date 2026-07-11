//! LLM provider configuration: platform config directory, on-disk schema,
//! loading/merging/validation, and the interactive setup wizard.
//!
//! Config lives outside the project directory in the platform-standard
//! config location (see [`paths::config_dir`]). The bundled `llm.toml` is
//! only a template and is never read by the program. Without a user profile,
//! translation asks the user to run `--setup`.

mod loader;
mod paths;
mod schema;
mod wizard;

pub use loader::{load_llm_profiles, materialize_llm_profile, resolve_llm_profile};
pub use paths::default_config_path;
pub(crate) use schema::{is_deepseek_endpoint, normalize_base_url};
pub use schema::{
    ApiFormat, LlmConfig, LlmConfigOverrides, LlmProfile, LlmProfileSnapshot, ThinkingEffort,
};
pub use wizard::run_setup_wizard;
