//! LLM provider configuration: platform config directory, on-disk schema,
//! loading/merging/validation, and the interactive setup wizard.
//!
//! Config lives outside the project directory, in the platform-standard
//! config location (see [`paths::config_dir`]). The project's bundled
//! `llm.toml` is only a human-readable template/example: it is never read
//! by the running program. With no user config file, callers fall back to
//! CLI flags plus built-in defaults; run with `--setup` to create a user
//! config file interactively.

mod loader;
mod paths;
mod schema;
mod wizard;

pub use loader::load_llm_config;
pub use paths::default_config_path;
pub use schema::{ApiFormat, LlmConfig, LlmConfigOverrides};
pub use wizard::run_setup_wizard;
