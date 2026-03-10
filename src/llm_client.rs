/// Anthropic Messages API client for paragraph translation.
use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};

use crate::types::LlmResponse;

const API_VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-sonnet-4-6";

fn api_url() -> String {
    let base = std::env::var("ANTHROPIC_BASE_URL")
        .unwrap_or_else(|_| "https://api.anthropic.com".to_string());
    format!("{}/v1/messages", base.trim_end_matches('/'))
}

// ── Request/Response shapes ─────────────────────────────────────────────────

#[derive(Serialize)]
struct ApiRequest {
    model: String,
    max_tokens: u32,
    messages: Vec<ApiMessage>,
    system: String,
}

#[derive(Serialize)]
struct ApiMessage {
    role: String,
    content: String,
}

#[derive(Deserialize)]
struct ApiResponse {
    content: Vec<ContentBlock>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

// ── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are an expert English-to-Chinese literary translator and English language teacher specializing in light novels.

For each paragraph the user sends, you MUST reply with a single JSON object (no markdown fences, no extra text) following this exact schema:

{
  "translation": "<中文翻译，自然流畅，保留原著风格>",
  "vocabulary": [
    {
      "word": "<英文单词或词组>",
      "ipa": "<IPA音标>",
      "pos": "<词性，如 n./v./adj./adv./phrase>",
      "cn": "<中文释义>",
      "example": "<英文例句（来自书本原句或自造）>"
    }
  ],
  "chunks": [
    {
      "chunk": "<常用短语/搭配/句型>",
      "cn": "<中文释义及用法说明>",
      "example": "<英文例句>"
    }
  ]
}

Rules:
1. "translation": translate the entire paragraph into natural, idiomatic Chinese.
2. "vocabulary": pick 3-8 words or phrases with IELTS difficulty ≥ 6.5 (C1/C2 level). Include academic/literary vocabulary, advanced idioms, and domain-specific terms worth noting. Skip common words.
3. "chunks": pick 2-5 useful collocations, fixed phrases, or syntactic patterns from the paragraph that are worth learning. Focus on native-sounding expressions.
4. Always output valid JSON. Escape any special characters properly.
5. If a paragraph is too short or lacks rich vocabulary, keep the arrays empty ([]).
"#;

// ── Public API ───────────────────────────────────────────────────────────────

pub struct LlmClient {
    client: Client,
    api_key: String,
}

impl LlmClient {
    pub fn new(api_key: String) -> Self {
        Self {
            client: Client::new(),
            api_key,
        }
    }

    /// Translate a single paragraph text, returning a structured LlmResponse.
    /// Retries up to 3 times on transient errors.
    pub async fn translate_paragraph(&self, text: &str) -> Result<LlmResponse> {
        let mut last_err = anyhow::anyhow!("no attempts made");

        for attempt in 1..=3 {
            match self.call_api(text).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    eprintln!(
                        "  [llm] attempt {}/3 failed: {}",
                        attempt,
                        e
                    );
                    last_err = e;
                    tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
                }
            }
        }
        Err(last_err)
    }

    async fn call_api(&self, paragraph_text: &str) -> Result<LlmResponse> {
        let req_body = ApiRequest {
            model: MODEL.to_string(),
            max_tokens: 2048,
            system: SYSTEM_PROMPT.to_string(),
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content: paragraph_text.to_string(),
            }],
        };

        let resp = self
            .client
            .post(api_url())
            .header("x-api-key", &self.api_key)
            .header("anthropic-version", API_VERSION)
            .header("content-type", "application/json")
            .json(&req_body)
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("API error {}: {}", status, body);
        }

        let api_resp: ApiResponse = resp.json().await.context("failed to parse API response")?;

        let text = api_resp
            .content
            .into_iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");

        // Strip markdown code fences if the model adds them
        let json_str = strip_code_fences(&text);

        let llm_resp: LlmResponse =
            serde_json::from_str(json_str).context("LLM returned invalid JSON")?;

        Ok(llm_resp)
    }
}

fn strip_code_fences(s: &str) -> &str {
    let s = s.trim();
    // ```json ... ``` or ``` ... ```
    if let Some(inner) = s.strip_prefix("```json") {
        if let Some(inner2) = inner.strip_suffix("```") {
            return inner2.trim();
        }
    }
    if let Some(inner) = s.strip_prefix("```") {
        if let Some(inner2) = inner.strip_suffix("```") {
            return inner2.trim();
        }
    }
    s
}
