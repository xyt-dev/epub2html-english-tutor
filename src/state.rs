/// State management for resumable LLM processing.
///
/// State file lives at `output/{book_slug}_state.json` and stores a map of
/// `{ para_id → LlmResponse }`.  On crash / interrupt the program re-reads
/// the state file, skips already-done paragraphs, and continues from the
/// first pending one.
use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::types::LlmResponse;

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct State {
    /// paragraph id → completed LLM response（BTreeMap 保证序列化顺序按 ID 字典序）
    pub completed: BTreeMap<String, LlmResponse>,
}

impl State {
    pub fn is_done(&self, para_id: &str) -> bool {
        self.completed.contains_key(para_id)
    }

    pub fn mark_done(&mut self, para_id: String, resp: LlmResponse) {
        self.completed.insert(para_id, resp);
    }

    pub fn get(&self, para_id: &str) -> Option<&LlmResponse> {
        self.completed.get(para_id)
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
    std::fs::write(path, content)?;
    Ok(())
}
