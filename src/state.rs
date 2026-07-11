/// State management for resumable LLM processing.
///
/// State lives at `output/{book_slug}_state.json`. Its first field records
/// the effective non-secret LLM configuration used by this book; `completed`
/// stores `{ para_id → LlmResponse }`. Old state files containing only
/// `completed` remain valid.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::config::LlmProfileSnapshot;
use crate::types::LlmResponse;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// Actual resolved configuration for this book. API keys are never stored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub llm: Option<LlmProfileSnapshot>,
    /// paragraph id → completed LLM response（BTreeMap 保证序列化顺序按 ID 字典序）
    #[serde(default)]
    pub completed: BTreeMap<String, LlmResponse>,
}

impl State {
    pub fn is_done(&self, para_id: &str) -> bool {
        self.completed.contains_key(para_id)
    }

    pub fn mark_done(&mut self, para_id: String, resp: LlmResponse) {
        self.completed.insert(para_id, resp);
    }
}

pub fn state_path(output_dir: &Path, book_slug: &str) -> PathBuf {
    output_dir.join(format!("{}_state.json", book_slug))
}

pub fn load_state(path: &Path) -> Result<State> {
    if path.exists() {
        let content = std::fs::read_to_string(path)?;
        let state: State = serde_json::from_str(&content)?;
        Ok(state)
    } else {
        Ok(State::default())
    }
}

pub fn save_state(path: &Path, state: &State) -> Result<()> {
    let content = serde_json::to_string_pretty(state)?;
    crate::fs_utils::atomic_write(path, content.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ThinkingEffort;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn temp_state_path() -> PathBuf {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "epub_reader_state_{}_{}.json",
            std::process::id(),
            n
        ))
    }

    fn snapshot() -> LlmProfileSnapshot {
        LlmProfileSnapshot {
            name: "saved-profile".to_string(),
            provider: "openai".to_string(),
            model: "saved-model".to_string(),
            base_url: "https://api.deepseek.com".to_string(),
            thinking: true,
            thinking_effort: ThinkingEffort::Max,
            max_output_tokens: 8192,
            request_timeout_secs: 180,
            api_key_env: "DEEPSEEK_API_KEY".to_string(),
            jobs: 4,
        }
    }

    #[test]
    fn loads_legacy_state_without_llm_header() {
        let path = temp_state_path();
        std::fs::write(
            &path,
            r#"{"completed":{"p1":{"translation":"译文","vocabulary":[],"chunks":[]}}}"#,
        )
        .unwrap();

        let state = load_state(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(state.llm.is_none());
        assert_eq!(state.completed.len(), 1);
        assert_eq!(state.completed["p1"].translation, "译文");
    }

    #[test]
    fn rewrites_legacy_budget_field_out_of_state() {
        let path = temp_state_path();
        std::fs::write(
            &path,
            r#"{
  "llm": {
    "name": "legacy",
    "provider": "anthropic",
    "model": "claude-sonnet-5",
    "base_url": "apiclaude.cc",
    "thinking": true,
    "thinking_effort": "max",
    "thinking_budget_tokens": 4096,
    "max_output_tokens": 8192,
    "request_timeout_secs": 180,
    "api_key_env": "ANTHROPIC_AUTH_TOKEN",
    "jobs": 2
  },
  "completed": {}
}"#,
        )
        .unwrap();

        let state = load_state(&path).unwrap();
        save_state(&path, &state).unwrap();
        let rewritten = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(!rewritten.contains("thinking_budget"), "{rewritten}");
    }

    #[test]
    fn saves_effective_config_first_without_api_key() {
        let path = temp_state_path();
        let mut state = State {
            llm: Some(snapshot()),
            completed: BTreeMap::new(),
        };
        state.mark_done(
            "p1".to_string(),
            LlmResponse {
                translation: "translated".to_string(),
                vocabulary: Vec::new(),
                chunks: Vec::new(),
            },
        );

        save_state(&path, &state).unwrap();
        let raw = std::fs::read_to_string(&path).unwrap();
        let loaded = load_state(&path).unwrap();
        let _ = std::fs::remove_file(&path);

        assert!(raw.starts_with("{\n  \"llm\": {"), "{raw}");
        assert!(raw.contains("\"api_key_env\": \"DEEPSEEK_API_KEY\""));
        assert!(!raw.contains("\"api_key\":"), "{raw}");
        assert_eq!(loaded.llm.as_ref().unwrap(), &snapshot());
        assert_eq!(loaded.completed["p1"].translation, "translated");
    }

    #[test]
    fn keeps_existing_state_path_convention() {
        assert_eq!(
            state_path(Path::new("output"), "sample-book"),
            Path::new("output").join("sample-book_state.json")
        );
    }
}
