/// Multi-provider LLM client for paragraph translation.
///
/// Supports two wire formats, chosen via `LlmConfig::format`:
/// - `Anthropic`: the Anthropic Messages API (`/v1/messages`), also spoken by
///   most Anthropic-compatible relay gateways (中转站).
/// - `OpenAi`: the OpenAI Chat Completions API (`/chat/completions`), also
///   spoken by DeepSeek's native API.
use anyhow::{bail, Context, Result};
use reqwest::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::config::{is_deepseek_endpoint, ApiFormat, LlmConfig};
use crate::types::LlmResponse;

const ANTHROPIC_API_VERSION: &str = "2023-06-01";

// ── System prompt ────────────────────────────────────────────────────────────

const SYSTEM_PROMPT: &str = r#"You are an expert English-to-Chinese literary translator and English language teacher specializing in light novels.

Return exactly one JSON object with this schema:

{
  "items": [
    {
      "id": "<copy the input id exactly>",
      "translation": "<中文翻译，按轻小说中文本地化风格处理；不要逐字硬译，要根据上下文用中文重新说顺、说活。允许适度加戏：强化语气、节奏、吐槽、暧昧张力、画面感和文学意境，但不要改变剧情事实、人物关系或段落核心信息>",
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
2. "translation": translate the full paragraph naturally and preserve the original tone; prefer expressive Chinese localization over literal wording, with tasteful embellishment when it improves voice, rhythm, humor, tension, or imagery.
3. "vocabulary": pick 0-10 advanced words or phrases worth learning (about IELTS 6.5+, C1/C2). Skip common words.
4. "chunks": pick 0-10 useful collocations, phrases, or sentence patterns worth learning.
5. If a paragraph is too short or lacks rich material, keep "vocabulary" and "chunks" as [].
6. Output valid JSON only. No markdown fences, no notes, no omitted ids.
7. Every input "text" field is the complete paragraph. Never ask for more text.
8. The "book.title" field identifies the source book and should guide title-specific terminology and tone.
9. The optional "context" array contains earlier source paragraphs only for continuity, pronouns, tone, and terminology. Do not translate context items and do not include their ids in the output.
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

pub fn estimate_translation_input_tokens(
    book_title: &str,
    context: &[TranslationRequest<'_>],
    items: &[TranslationRequest<'_>],
) -> usize {
    let content = serialize_batch_input(book_title, context, items)
        .expect("serializing translation batch input should not fail");
    crate::token_estimator::estimate_message_input_tokens(SYSTEM_PROMPT, &content)
}

#[derive(Clone)]
pub struct LlmClient {
    client: Client,
    config: LlmConfig,
    verbose: bool,
}

impl LlmClient {
    pub fn new(config: LlmConfig, verbose: bool) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(config.request_timeout_secs))
            .build()
            .expect("failed to build HTTP client");
        Self {
            client,
            config,
            verbose,
        }
    }

    /// Translate one or more paragraphs in a single request.
    /// Retries up to 3 times on transient errors.
    pub async fn translate_batch(
        &self,
        book_title: &str,
        context: &[TranslationRequest<'_>],
        items: &[TranslationRequest<'_>],
    ) -> Result<Vec<TranslationResult>> {
        if items.is_empty() {
            return Ok(Vec::new());
        }

        let mut last_err = anyhow::anyhow!("no attempts made");

        for attempt in 1..=3 {
            match self.call_api(book_title, context, items).await {
                Ok(resp) => return Ok(resp),
                Err(e) => {
                    let retryable = is_retryable_translation_error(&e);
                    if self.verbose {
                        eprintln!("  [llm] attempt {}/3 failed: {:#}", attempt, e);
                    } else {
                        eprintln!(
                            "  [llm] attempt {}/3 failed: {} (rerun with --verbose for the full response dump)",
                            attempt,
                            first_line(&format!("{:#}", e))
                        );
                    }
                    if !retryable {
                        return Err(e);
                    }
                    last_err = e;
                    tokio::time::sleep(std::time::Duration::from_secs(2 * attempt)).await;
                }
            }
        }
        Err(last_err)
    }

    async fn call_api(
        &self,
        book_title: &str,
        context: &[TranslationRequest<'_>],
        items: &[TranslationRequest<'_>],
    ) -> Result<Vec<TranslationResult>> {
        let content = serialize_batch_input(book_title, context, items)?;
        let max_tokens = self.config.max_output_tokens;
        let url = self.api_url();
        let req_body = self.build_request_body(&content, max_tokens);

        if self.verbose {
            print_verbose_request(&url, &req_body);
        }

        let mut request = self
            .client
            .post(&url)
            .header("content-type", "application/json");
        request = match self.config.format {
            ApiFormat::Anthropic => request
                .header("x-api-key", &self.config.api_key)
                .header("anthropic-version", ANTHROPIC_API_VERSION),
            ApiFormat::OpenAi => {
                request.header("Authorization", format!("Bearer {}", self.config.api_key))
            }
        };

        let resp = request
            .json(&req_body)
            .send()
            .await
            .context("HTTP request failed")?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            bail!("API error {}: {}", status, body);
        }

        let response_body = resp.text().await.context("failed to read API response")?;
        if self.verbose {
            print_verbose_response(status.as_u16(), &response_body);
        }

        let text = self.extract_completion_text(&response_body, max_tokens)?;
        parse_batch_response(&text, items)
    }

    fn api_url(&self) -> String {
        let base = self.config.base_url.trim_end_matches('/');
        match self.config.format {
            ApiFormat::Anthropic => format!("{}/v1/messages", base),
            ApiFormat::OpenAi => format!("{}/chat/completions", base),
        }
    }

    fn build_request_body(&self, content: &str, max_tokens: u32) -> Value {
        match self.config.format {
            ApiFormat::Anthropic => {
                let mut body = json!({
                    "model": self.config.model,
                    "max_tokens": max_tokens,
                    "system": SYSTEM_PROMPT,
                    "messages": [{"role": "user", "content": content}],
                });
                let deepseek = is_deepseek_endpoint(&self.config.base_url);
                body["thinking"] = json!({
                    "type": match (deepseek, self.config.thinking) {
                        (true, true) => "enabled",
                        (false, true) => "adaptive",
                        (_, false) => "disabled",
                    },
                });
                if self.config.thinking {
                    let effort = if deepseek {
                        self.config.thinking_effort.normalized_deepseek()
                    } else {
                        self.config.thinking_effort.as_str()
                    };
                    body["output_config"] = json!({ "effort": effort });
                }
                body
            }
            ApiFormat::OpenAi => {
                let mut body = json!({
                    "model": self.config.model,
                    "max_tokens": max_tokens,
                    "messages": [
                        {"role": "system", "content": SYSTEM_PROMPT},
                        {"role": "user", "content": content},
                    ],
                    "thinking": {
                        "type": if self.config.thinking { "enabled" } else { "disabled" },
                    },
                });
                if self.config.thinking {
                    body["reasoning_effort"] =
                        json!(self.config.thinking_effort.normalized_deepseek());
                }
                body
            }
        }
    }

    fn extract_completion_text(&self, response_body: &str, max_tokens: u32) -> Result<String> {
        match self.config.format {
            ApiFormat::Anthropic => {
                let api_resp: AnthropicResponse =
                    serde_json::from_str(response_body).context("failed to parse API response")?;

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

                if api_resp.stop_reason.as_deref() == Some("max_tokens") {
                    bail!(
                        "API stopped at max_tokens ({}) after {} response chars; batch JSON is likely truncated. Reduce batch size or raise max_output_tokens in llm.toml if the provider supports it.",
                        max_tokens,
                        text.len(),
                    );
                }

                Ok(text)
            }
            ApiFormat::OpenAi => {
                let api_resp: OpenAiResponse =
                    serde_json::from_str(response_body).context("failed to parse API response")?;

                let choice = api_resp
                    .choices
                    .into_iter()
                    .next()
                    .context("API returned no choices")?;
                let text = choice.message.content.unwrap_or_default();

                if text.is_empty() {
                    bail!("API returned empty content (batch likely blocked by content filter)");
                }

                if choice.finish_reason.as_deref() == Some("length") {
                    bail!(
                        "API stopped at max_tokens ({}) after {} response chars; batch JSON is likely truncated. Reduce batch size or raise max_output_tokens in llm.toml if the provider supports it.",
                        max_tokens,
                        text.len(),
                    );
                }

                Ok(text)
            }
        }
    }
}

// ── Response shapes ──────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct AnthropicResponse {
    content: Option<Vec<AnthropicContentBlock>>,
    stop_reason: Option<String>,
}

#[derive(Deserialize)]
struct AnthropicContentBlock {
    #[serde(rename = "type")]
    block_type: String,
    text: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiResponse {
    choices: Vec<OpenAiChoice>,
}

#[derive(Deserialize)]
struct OpenAiChoice {
    message: OpenAiMessage,
    finish_reason: Option<String>,
}

#[derive(Deserialize)]
struct OpenAiMessage {
    content: Option<String>,
}

fn serialize_batch_input(
    book_title: &str,
    context: &[TranslationRequest<'_>],
    items: &[TranslationRequest<'_>],
) -> Result<String> {
    serde_json::to_string(&BatchInput {
        book: BatchBook { title: book_title },
        context: context
            .iter()
            .map(|item| BatchInputItem {
                id: item.id,
                text: item.text,
            })
            .collect(),
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

fn print_verbose_request(url: &str, req_body: &Value) {
    let json_body = serde_json::to_string_pretty(req_body)
        .unwrap_or_else(|err| format!("<failed to serialize request body: {}>", err));

    eprintln!(
        "\n========== LLM REQUEST ==========\nPOST {}\n{}\n========== END LLM REQUEST ==========\n",
        url, json_body,
    );
}

fn print_verbose_response(status: u16, response_body: &str) {
    eprintln!(
        "\n========== LLM RESPONSE ==========\nstatus: {}\n--- raw body ---\n{}\n========== END LLM RESPONSE ==========\n",
        status, response_body
    );
}

fn is_retryable_translation_error(err: &anyhow::Error) -> bool {
    let message = format!("{:#}", err);
    !message.contains("API stopped at max_tokens")
}

/// First line of a (possibly multi-line) error message, for concise
/// non-verbose logging.
fn first_line(message: &str) -> &str {
    message.split('\n').next().unwrap_or(message)
}

#[derive(serde::Serialize)]
struct BatchInput<'a> {
    book: BatchBook<'a>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    context: Vec<BatchInputItem<'a>>,
    items: Vec<BatchInputItem<'a>>,
}

#[derive(serde::Serialize)]
struct BatchBook<'a> {
    title: &'a str,
}

#[derive(serde::Serialize)]
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

    let payload: BatchOutput = match serde_json::from_str(&json_str) {
        Ok(payload) => payload,
        Err(err) => {
            let json_head = truncate_str(&json_str, 900);
            let json_tail = truncate_tail_str(&json_str, 900);
            let raw_preview = truncate_str(raw, 240);
            bail!(
                "LLM returned invalid batch JSON: {}.\nExtracted ({} chars, first 900):\n---\n{}\n---\nExtracted tail (last 900):\n---\n{}\n---\nRaw ({} chars, first 240):\n---\n{}\n---",
                err,
                json_str.len(),
                json_head,
                json_tail,
                raw.len(),
                raw_preview,
            );
        }
    };

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

/// Keep the tail of a string without splitting a UTF-8 character.
fn truncate_tail_str(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = s.len() - max_bytes;
    while !s.is_char_boundary(boundary) {
        boundary += 1;
    }
    &s[boundary..]
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
    use crate::config::ThinkingEffort;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn test_llm_config(format: ApiFormat, base_url: String) -> LlmConfig {
        LlmConfig {
            profile_name: "test-profile".to_string(),
            format,
            model: "test-model".to_string(),
            base_url,
            thinking: false,
            thinking_effort: crate::config::ThinkingEffort::High,
            max_output_tokens: 1024,
            request_timeout_secs: 5,
            api_key: "test-key".to_string(),
            jobs: 2,
        }
    }

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

    fn anthropic_success_response(text: &str) -> Value {
        json!({
            "content": [{"type": "text", "text": text}],
            "stop_reason": "end_turn"
        })
    }

    fn openai_success_response(text: &str) -> Value {
        json!({
            "choices": [{"message": {"content": text}, "finish_reason": "stop"}]
        })
    }

    #[test]
    fn anthropic_request_explicitly_disables_thinking() {
        let config = test_llm_config(ApiFormat::Anthropic, "http://example.invalid".to_string());
        let client = LlmClient::new(config, false);
        let body = client.build_request_body("content", 1024);
        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn anthropic_request_uses_official_effort_without_budget() {
        let mut config =
            test_llm_config(ApiFormat::Anthropic, "http://example.invalid".to_string());
        config.thinking = true;
        config.thinking_effort = ThinkingEffort::Medium;
        let client = LlmClient::new(config, false);
        let body = client.build_request_body("content", 4096);
        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body["thinking"].get("budget_tokens").is_none());
        assert_eq!(body["output_config"]["effort"], "medium");
    }

    #[test]
    fn claude_sonnet_5_uses_adaptive_thinking_and_exact_effort() {
        let mut config = test_llm_config(ApiFormat::Anthropic, "https://apiclaude.cc".to_string());
        config.model = "claude-sonnet-5".to_string();
        config.thinking = true;
        config.thinking_effort = ThinkingEffort::Max;
        let client = LlmClient::new(config, false);

        let body = client.build_request_body("content", 8192);

        assert_eq!(body["thinking"]["type"], "adaptive");
        assert!(body["thinking"].get("budget_tokens").is_none());
        assert_eq!(body["output_config"]["effort"], "max");
    }

    #[test]
    fn claude_sonnet_5_explicitly_disables_default_adaptive_thinking() {
        let mut config = test_llm_config(ApiFormat::Anthropic, "https://apiclaude.cc".to_string());
        config.model = "claude-sonnet-5".to_string();
        config.thinking = false;
        let client = LlmClient::new(config, false);

        let body = client.build_request_body("content", 8192);

        assert_eq!(body["thinking"]["type"], "disabled");
        assert!(body.get("output_config").is_none());
    }

    #[test]
    fn deepseek_anthropic_request_includes_normalized_effort() {
        let mut config = test_llm_config(
            ApiFormat::Anthropic,
            "https://api.deepseek.com/anthropic".to_string(),
        );
        config.thinking = true;
        config.thinking_effort = ThinkingEffort::XHigh;
        let client = LlmClient::new(config, false);
        let body = client.build_request_body("content", 4096);

        assert_eq!(body["thinking"]["type"], "enabled");
        assert_eq!(body["output_config"]["effort"], "max");
        assert!(body["thinking"].get("budget_tokens").is_none());
    }

    #[test]
    fn openai_request_thinking_type_and_effort_reflect_config() {
        let mut enabled_config =
            test_llm_config(ApiFormat::OpenAi, "http://example.invalid".to_string());
        enabled_config.thinking = true;
        enabled_config.thinking_effort = ThinkingEffort::Medium;
        let enabled_client = LlmClient::new(enabled_config, false);
        let enabled_body = enabled_client.build_request_body("content", 4096);
        assert_eq!(enabled_body["thinking"]["type"], "enabled");
        assert_eq!(enabled_body["reasoning_effort"], "high");

        let disabled_config =
            test_llm_config(ApiFormat::OpenAi, "http://example.invalid".to_string());
        let disabled_client = LlmClient::new(disabled_config, false);
        let disabled_body = disabled_client.build_request_body("content", 4096);
        assert_eq!(disabled_body["thinking"]["type"], "disabled");
        assert!(disabled_body.get("reasoning_effort").is_none());
    }

    #[test]
    fn deepseek_effort_levels_are_normalized_to_supported_values() {
        let cases = [
            (ThinkingEffort::Low, "high"),
            (ThinkingEffort::Medium, "high"),
            (ThinkingEffort::High, "high"),
            (ThinkingEffort::XHigh, "max"),
            (ThinkingEffort::Max, "max"),
        ];

        for (effort, expected) in cases {
            assert_eq!(effort.normalized_deepseek(), expected);
        }
    }

    #[tokio::test]
    async fn translate_batch_retries_on_transient_5xx_then_succeeds() {
        let mock_server = MockServer::start().await;
        let config = test_llm_config(ApiFormat::Anthropic, mock_server.uri());
        let client = LlmClient::new(config, false);

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(anthropic_success_response(
                    r#"{"items":[{"id":"p1","translation":"你好","vocabulary":[],"chunks":[]}]}"#,
                )),
            )
            .mount(&mock_server)
            .await;

        let items = [TranslationRequest {
            id: "p1",
            text: "hello",
        }];
        let result = client.translate_batch("Book", &[], &items).await.unwrap();

        assert_eq!(result.len(), 1);
        assert_eq!(result[0].response.translation, "你好");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn translate_batch_retries_on_malformed_json_then_succeeds() {
        let mock_server = MockServer::start().await;
        let config = test_llm_config(ApiFormat::Anthropic, mock_server.uri());
        let client = LlmClient::new(config, false);

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(anthropic_success_response(
                    "this is not json at all, the model went off the rails",
                )),
            )
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(anthropic_success_response(
                    r#"{"items":[{"id":"p1","translation":"你好","vocabulary":[],"chunks":[]}]}"#,
                )),
            )
            .mount(&mock_server)
            .await;

        let items = [TranslationRequest {
            id: "p1",
            text: "hello",
        }];
        let result = client.translate_batch("Book", &[], &items).await.unwrap();

        assert_eq!(result[0].response.translation, "你好");
        assert_eq!(mock_server.received_requests().await.unwrap().len(), 2);
    }

    #[tokio::test]
    async fn translate_batch_gives_up_after_three_attempts() {
        let mock_server = MockServer::start().await;
        let config = test_llm_config(ApiFormat::Anthropic, mock_server.uri());
        let client = LlmClient::new(config, false);

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(anthropic_success_response("still not json")),
            )
            .mount(&mock_server)
            .await;

        let items = [TranslationRequest {
            id: "p1",
            text: "hello",
        }];
        let err = client
            .translate_batch("Book", &[], &items)
            .await
            .unwrap_err();

        assert!(format!("{:#}", err).contains("invalid batch JSON"));
        assert_eq!(mock_server.received_requests().await.unwrap().len(), 3);
    }

    #[tokio::test]
    async fn translate_batch_does_not_retry_when_truncated_by_max_tokens() {
        let mock_server = MockServer::start().await;
        let config = test_llm_config(ApiFormat::Anthropic, mock_server.uri());
        let client = LlmClient::new(config, false);

        Mock::given(method("POST"))
            .and(path("/v1/messages"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "content": [{"type": "text", "text": "{\"items\":["}],
                "stop_reason": "max_tokens"
            })))
            .mount(&mock_server)
            .await;

        let items = [TranslationRequest {
            id: "p1",
            text: "hello",
        }];
        let err = client
            .translate_batch("Book", &[], &items)
            .await
            .unwrap_err();

        assert!(format!("{:#}", err).contains("API stopped at max_tokens"));
        assert_eq!(mock_server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn translate_batch_openai_format_retries_then_succeeds() {
        let mock_server = MockServer::start().await;
        let config = test_llm_config(ApiFormat::OpenAi, mock_server.uri());
        let client = LlmClient::new(config, false);

        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(ResponseTemplate::new(503))
            .up_to_n_times(1)
            .mount(&mock_server)
            .await;
        Mock::given(method("POST"))
            .and(path("/chat/completions"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(openai_success_response(
                    r#"{"items":[{"id":"p1","translation":"你好","vocabulary":[],"chunks":[]}]}"#,
                )),
            )
            .mount(&mock_server)
            .await;

        let items = [TranslationRequest {
            id: "p1",
            text: "hello",
        }];
        let result = client.translate_batch("Book", &[], &items).await.unwrap();

        assert_eq!(result[0].response.translation, "你好");
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

    #[test]
    fn serialized_batch_input_includes_book_title() {
        let items = [TranslationRequest {
            id: "p1",
            text: "first",
        }];

        let raw = serialize_batch_input("Sample Book", &[], &items).unwrap();
        let value: serde_json::Value = serde_json::from_str(&raw).unwrap();

        assert_eq!(value["book"]["title"], "Sample Book");
        assert_eq!(value["items"][0]["id"], "p1");
    }

    #[test]
    fn max_tokens_errors_are_not_retried_as_same_batch() {
        let err = anyhow::anyhow!(
            "API stopped at max_tokens after 7000 response chars; batch JSON is likely truncated"
        );

        assert!(!is_retryable_translation_error(&err));
    }
}
