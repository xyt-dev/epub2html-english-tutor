//! Platform-standard config directory resolution.

use anyhow::{Context, Result};
use std::path::PathBuf;

/// Config subdirectory name under the platform config root.
pub const APP_NAME: &str = "epub-reader";

const CONFIG_FILE_NAME: &str = "llm.toml";

/// Platform config directory for this app:
/// - Linux: `$XDG_CONFIG_HOME` or `~/.config/epub-reader`
/// - macOS: `~/Library/Application Support/epub-reader`
/// - Windows: `%APPDATA%\epub-reader`
pub fn config_dir() -> Result<PathBuf> {
    dirs::config_dir()
        .map(|dir| dir.join(APP_NAME))
        .context("could not determine the platform config directory (no home directory?)")
}

/// Default path to the LLM config file inside the platform config dir.
pub fn default_config_path() -> Result<PathBuf> {
    Ok(config_dir()?.join(CONFIG_FILE_NAME))
}
