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
6. CRITICAL: You will NEVER ask for clarification, never say the message is cut off, and never ask for more text. The paragraph between <paragraph> and </paragraph> tags is ALWAYS the complete input. Always respond with the JSON object, no matter what.
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
            max_tokens: 4096,
            system: SYSTEM_PROMPT.to_string(),
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content: format!("<paragraph>{}</paragraph>", paragraph_text),
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

        // Strip markdown code fences if the model adds them, then extract JSON object
        let json_str = extract_json(&text);

        let llm_resp: LlmResponse = serde_json::from_str(&json_str)
            .with_context(|| {
                let json_preview = truncate_str(&json_str, 600);
                let raw_preview = truncate_str(&text, 200);
                format!(
                    "LLM returned invalid JSON.\nExtracted ({} chars):\n---\n{}\n---\nRaw ({} chars, first 200):\n---\n{}\n---",
                    json_str.len(),
                    json_preview,
                    text.len(),
                    raw_preview,
                )
            })?;

        Ok(llm_resp)
    }
}

/// Best-effort extraction of a JSON object from LLM output.
/// Handles: plain JSON, ```json fences, stray text before/after the object,
/// and unescaped double-quotes inside string values (e.g. Chinese dialogue marks).
fn extract_json(raw: &str) -> String {
    let s = raw.trim();

    // 1. Strip code fences using rfind to locate the closing ``` correctly.
    //    trim_end_matches("```") fails when the LLM puts a newline after the
    //    closing fence (e.g. "...\n}\n```\n"), because the string ends with \n.
    let stripped = strip_code_fence(s);

    // 2. If it parses cleanly now, return it
    if serde_json::from_str::<serde_json::Value>(stripped).is_ok() {
        return stripped.to_string();
    }

    // 3. Try repairs, then recheck
    let repaired = repair(stripped);
    if serde_json::from_str::<serde_json::Value>(&repaired).is_ok() {
        return repaired;
    }

    // 4. Scan for first '{' and match its closing '}' by depth
    let bytes = stripped.as_bytes();
    if let Some(start) = bytes.iter().position(|&b| b == b'{') {
        let mut depth = 0usize;
        let mut in_str = false;
        let mut escape = false;
        for (i, &b) in bytes[start..].iter().enumerate() {
            if escape {
                escape = false;
                continue;
            }
            match b {
                b'\\' if in_str => escape = true,
                b'"' => in_str = !in_str,
                b'{' if !in_str => depth += 1,
                b'}' if !in_str => {
                    depth -= 1;
                    if depth == 0 {
                        let candidate = &stripped[start..start + i + 1];
                        let repaired2 = repair(candidate);
                        if serde_json::from_str::<serde_json::Value>(&repaired2).is_ok() {
                            return repaired2;
                        }
                        return candidate.to_string();
                    }
                }
                _ => {}
            }
        }
    }

    // 5. Fallback: return stripped as-is (will fail JSON parse with a useful error)
    stripped.to_string()
}

/// Apply all known LLM JSON output repairs in sequence.
fn repair(s: &str) -> String {
    let s = repair_missing_colon(s);
    repair_unescaped_quotes(&s)
}

/// Fix `"key"[` or `"key"{` → `"key":[` / `"key":{`
/// The LLM occasionally omits the `:` between a key and its array/object value.
fn repair_missing_colon(s: &str) -> String {
    // Simple byte scan: when outside a string we look for `"` immediately
    // followed (ignoring spaces) by `[` or `{` — insert `:` between them.
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = Vec::with_capacity(n + 8);
    let mut i = 0;
    let mut in_str = false;
    let mut escape = false;

    while i < n {
        let b = bytes[i];

        if escape {
            escape = false;
            out.push(b);
            i += 1;
            continue;
        }
        if b == b'\\' && in_str {
            escape = true;
            out.push(b);
            i += 1;
            continue;
        }
        if b == b'"' {
            in_str = !in_str;
            out.push(b);
            i += 1;
            // After closing a string key, peek ahead for missing colon
            if !in_str {
                let mut j = i;
                while j < n && matches!(bytes[j], b' ' | b'\t' | b'\r' | b'\n') {
                    j += 1;
                }
                if j < n && matches!(bytes[j], b'[' | b'{') {
                    out.push(b':');
                }
            }
            continue;
        }

        out.push(b);
        i += 1;
    }

    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Repair unescaped double-quotes inside JSON string values.
///
/// The LLM sometimes emits literal `"` characters inside string values without
/// escaping them (e.g. `"translation": "She said "hello" to him"`).  We walk
/// the raw bytes with a state machine:
///   • outside a string  → `"` opens a string
///   • inside a string   → `\` sets escape; then check if an unescaped `"` is a
///                          genuine closing quote (next non-whitespace is `,` `:` `}` `]`)
///                          or a spurious quote that should be escaped.
fn repair_unescaped_quotes(s: &str) -> String {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut out = Vec::with_capacity(n + 64);
    let mut i = 0;
    let mut in_str = false;
    let mut escape = false;

    while i < n {
        let b = bytes[i];

        if escape {
            escape = false;
            out.push(b);
            i += 1;
            continue;
        }

        if b == b'\\' && in_str {
            escape = true;
            out.push(b);
            i += 1;
            continue;
        }

        if b == b'"' {
            if !in_str {
                // Opening a string
                in_str = true;
                out.push(b);
            } else {
                // Could be closing the string OR an unescaped quote inside it.
                // Look ahead past whitespace to see if the next non-space char
                // is a JSON value terminator: , : } ]
                let mut j = i + 1;
                while j < n && matches!(bytes[j], b' ' | b'\t' | b'\r' | b'\n') {
                    j += 1;
                }
                let next = if j < n { bytes[j] } else { 0 };
                if matches!(next, b',' | b':' | b'}' | b']' | 0) {
                    // Genuine closing quote
                    in_str = false;
                    out.push(b);
                } else {
                    // Unescaped quote inside value — escape it
                    out.push(b'\\');
                    out.push(b'"');
                }
            }
        } else {
            out.push(b);
        }

        i += 1;
    }

    // SAFETY: we only copied bytes from a valid UTF-8 string and inserted ASCII
    // escape sequences, so the result is still valid UTF-8.
    String::from_utf8(out).unwrap_or_else(|_| s.to_string())
}

/// Truncate a string to at most `max_bytes` bytes without splitting a UTF-8 character.
fn truncate_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = max_bytes;
    while !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

/// Strip ```json or ``` fences, using rfind for the closing fence so that a
/// trailing newline after the closing ``` doesn't break the extraction.
fn strip_code_fence(s: &str) -> &str {
    for prefix in &["```json", "```"] {
        if let Some(after_open) = s.strip_prefix(prefix) {
            // Remove the leading newline that follows the opening fence
            let content = after_open.trim_start_matches('\n');
            // Find the last ``` (the closing fence) and take everything before it
            return if let Some(close) = content.rfind("```") {
                content[..close].trim()
            } else {
                // No closing fence: the whole remainder is the JSON (truncated response)
                content.trim()
            };
        }
    }
    s
}
