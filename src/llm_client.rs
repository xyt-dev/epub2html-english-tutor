/// Anthropic Messages API client for paragraph translation.
use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

use crate::types::LlmResponse;

const API_VERSION: &str = "2023-06-01";
const MODEL: &str = "claude-sonnet-4-6";
const MAX_OUTPUT_TOKENS: u32 = 8_192;

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
    content: Option<Vec<ContentBlock>>,
}

#[derive(Deserialize)]
struct ContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

// ── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are an expert English-to-Chinese literary translator and English language teacher specializing in light novels.

Return exactly one JSON object with this schema:

{
  "items": [
    {
      "id": "<copy the input id exactly>",
      "translation": "<中文翻译，自然流畅，保留原著风格>",
      "vocabulary": [
        {
          "word": "<英文单词或词组>",
          "ipa": "<IPA音标>",
          "pos": "<词性，如 n./v./adj./adv./phrase>",
          "cn": "<中文释义>",
          "example": "<英文例句>"
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
  ]
}

Rules:
1. Process every input item and copy each "id" exactly once.
2. "translation": translate the full paragraph naturally and preserve the original tone.
3. "vocabulary": pick 3-8 advanced words or phrases worth learning (about IELTS 6.5+, C1/C2). Skip common words.
4. "chunks": pick 2-5 useful collocations, phrases, or sentence patterns worth learning.
5. If a paragraph is too short or lacks rich material, keep "vocabulary" and "chunks" as [].
6. Output valid JSON only. No markdown fences, no notes, no omitted ids.
7. Every input "text" field is the complete paragraph. Never ask for more text.
"#;

// ── Public API ───────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy)]
pub struct TranslationRequest<'a> {
    pub id: &'a str,
    pub text: &'a str,
}

#[derive(Debug, Clone)]
pub struct TranslationResult {
    pub id: String,
    pub response: LlmResponse,
}

pub fn estimate_translation_input_tokens(items: &[TranslationRequest<'_>]) -> usize {
    let content =
        serialize_batch_input(items).expect("serializing translation batch input should not fail");
    crate::token_estimator::estimate_message_input_tokens(SYSTEM_PROMPT, &content)
}

#[derive(Clone)]
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

    /// Translate one or more paragraphs in a single request.
    /// Retries up to 3 times on transient errors.
    pub async fn translate_batch(
        &self,
        items: &[TranslationRequest<'_>],
    ) -> Result<Vec<TranslationResult>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut last_err = anyhow::anyhow!("no attempts made");

        for attempt in 1..=3 {
            match self.call_api(items).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    eprintln!("  [llm] attempt {}/3 failed: {}", attempt, e);
                    last_err = e;
                    tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
                }
            }
        }
        Err(last_err)
    }

    pub async fn translate_paragraph(&self, para_id: &str, text: &str) -> Result<LlmResponse> {
        let request = [TranslationRequest { id: para_id, text }];
        let mut results = self.translate_batch(&request).await?;
        match results.len() {
            1 => Ok(results.pop().unwrap().response),
            count => bail!("expected 1 translation item, got {}", count),
        }
    }

    async fn call_api(&self, items: &[TranslationRequest<'_>]) -> Result<Vec<TranslationResult>> {
        let content = serialize_batch_input(items)?;

        let req_body = ApiRequest {
            model: MODEL.to_string(),
            max_tokens: MAX_OUTPUT_TOKENS,
            system: SYSTEM_PROMPT.to_string(),
            messages: vec![ApiMessage {
                role: "user".to_string(),
                content,
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
            .unwrap_or_default()
            .into_iter()
            .filter(|b| b.block_type == "text")
            .filter_map(|b| b.text)
            .collect::<Vec<_>>()
            .join("");

        if text.is_empty() {
            bail!("API returned empty content (batch likely blocked by content filter)");
        }

        parse_batch_response(&text, items)
    }
}

fn serialize_batch_input(items: &[TranslationRequest<'_>]) -> Result<String> {
    serde_json::to_string(&BatchInput {
        items: items
            .iter()
            .map(|item| BatchInputItem {
                id: item.id,
                text: item.text,
            })
            .collect(),
    })
    .context("failed to serialize translation batch request")
}

#[derive(Serialize)]
struct BatchInput<'a> {
    items: Vec<BatchInputItem<'a>>,
}

#[derive(Serialize)]
struct BatchInputItem<'a> {
    id: &'a str,
    text: &'a str,
}

#[derive(Deserialize)]
struct BatchOutput {
    items: Vec<BatchOutputItem>,
}

#[derive(Deserialize)]
struct BatchOutputItem {
    id: String,
    #[serde(flatten)]
    response: LlmResponse,
}

fn parse_batch_response(
    raw: &str,
    expected: &[TranslationRequest<'_>],
) -> Result<Vec<TranslationResult>> {
    let json_str = extract_json(raw);

    let payload: BatchOutput = serde_json::from_str(&json_str).with_context(|| {
        let json_preview = truncate_str(&json_str, 900);
        let raw_preview = truncate_str(raw, 240);
        format!(
            "LLM returned invalid batch JSON.\nExtracted ({} chars):\n---\n{}\n---\nRaw ({} chars, first 240):\n---\n{}\n---",
            json_str.len(),
            json_preview,
            raw.len(),
            raw_preview,
        )
    })?;

    validate_batch_items(payload.items, expected)
}

fn validate_batch_items(
    items: Vec<BatchOutputItem>,
    expected: &[TranslationRequest<'_>],
) -> Result<Vec<TranslationResult>> {
    let mut seen = HashSet::with_capacity(items.len());
    let mut by_id = HashMap::with_capacity(items.len());

    for item in items {
        if !seen.insert(item.id.clone()) {
            bail!("LLM returned duplicate id '{}'", item.id);
        }
        by_id.insert(item.id.clone(), item.response);
    }

    let mut ordered = Vec::with_capacity(expected.len());
    for request in expected {
        let response = by_id
            .remove(request.id)
            .with_context(|| format!("LLM response missing id '{}'", request.id))?;
        ordered.push(TranslationResult {
            id: request.id.to_string(),
            response,
        });
    }

    if !by_id.is_empty() {
        let mut unexpected = by_id.keys().cloned().collect::<Vec<_>>();
        unexpected.sort();
        bail!("LLM returned unexpected ids: {}", unexpected.join(", "));
    }

    Ok(ordered)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_response(id: &str, translation: &str) -> BatchOutputItem {
        BatchOutputItem {
            id: id.to_string(),
            response: LlmResponse {
                translation: translation.to_string(),
                vocabulary: Vec::new(),
                chunks: Vec::new(),
            },
        }
    }

    #[test]
    fn validate_batch_items_reorders_to_input_sequence() {
        let expected = [
            TranslationRequest {
                id: "p1",
                text: "first",
            },
            TranslationRequest {
                id: "p2",
                text: "second",
            },
        ];

        let items = vec![sample_response("p2", "two"), sample_response("p1", "one")];
        let ordered = validate_batch_items(items, &expected).unwrap();

        assert_eq!(ordered[0].id, "p1");
        assert_eq!(ordered[0].response.translation, "one");
        assert_eq!(ordered[1].id, "p2");
        assert_eq!(ordered[1].response.translation, "two");
    }

    #[test]
    fn validate_batch_items_rejects_missing_ids() {
        let expected = [
            TranslationRequest {
                id: "p1",
                text: "first",
            },
            TranslationRequest {
                id: "p2",
                text: "second",
            },
        ];

        let err = validate_batch_items(vec![sample_response("p1", "one")], &expected).unwrap_err();
        assert!(err.to_string().contains("missing id 'p2'"));
    }

    #[test]
    fn parse_batch_response_accepts_wrapped_json() {
        let expected = [TranslationRequest {
            id: "p1",
            text: "first",
        }];
        let raw = "```json\n{\"items\":[{\"id\":\"p1\",\"translation\":\"译文\",\"vocabulary\":[],\"chunks\":[]}]}\n```";

        let parsed = parse_batch_response(raw, &expected).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].id, "p1");
        assert_eq!(parsed[0].response.translation, "译文");
    }
}
