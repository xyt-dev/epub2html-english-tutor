# epub-reader — EPUB / Markdown / TXT to HTML + AI Paragraph Translation

[中文](README.md)

> Convert `.epub`, `.md/.markdown`, and `.txt` into readable HTML, then call an LLM (Anthropic-format API, including compatible relay gateways, or OpenAI-format API, including DeepSeek's native API) to generate a translation, vocabulary notes, and chunk analysis for each paragraph. Supports resume-after-interrupt, offline rebuild, controlled concurrency, contiguous paragraph batching, and configurable text segmentation.

![png](1.png)

## Features

- Supports `epub`, `md/markdown`, and `txt` input
- Works on a single file or recursively scans a directory
- Produces reader-friendly HTML with 3 collapsible AI sections per paragraph
- Preserves fenced Markdown code blocks and EPUB/HTML `<pre>` blocks in the output
- Code blocks are not sent for translation and are rendered with offline Catppuccin Mocha syntax highlighting
- Calls the configured LLM and expects structured JSON: translation / vocabulary / chunks, over either Anthropic format (incl. relay gateways) or OpenAI format (incl. DeepSeek)
- Sends contiguous paragraphs in batches and carries explicit paragraph IDs in both request and response payloads
- Supports `Ctrl+C` interrupt and resume without redoing completed paragraphs
- Supports `--rebuild` to regenerate HTML from state files without API calls
- Supports `--count` to count only text that would be sent to a third-party model, without API calls or output files
- Supports `--jobs` for concurrent requests and `--request-delay-ms` for throttling
- Supports `-v/--verbose` to print full LLM request and response contents in the terminal
- Sends the book title with every translation request and supports `--context-paragraphs` for the previous N source paragraphs
- Default batching strategy: target about `5000` effective chars, hard cap `7000`, max `10` paragraphs per request, with automatic single-paragraph fallback on batch failure
- TXT / Markdown segmentation behavior can be tuned from the CLI
- Generated HTML includes a chapter navigator, current-location badge, and paragraph-anchored resume
- Open/closed AI sections are persisted, and reading progress is computed by paragraph position rather than raw scroll height
- Both HTML and state files use atomic writes for safer crash recovery

## Installation

### Prerequisites

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# LLM settings (provider/model/base_url/thinking) live in llm.toml inside the
# platform-standard config directory (e.g. ~/.config/epub-reader/llm.toml on
# Linux). The project's own llm.toml is just a commented template; it is
# never read at runtime. Easiest way to set things up:
cargo run --release -- --setup
# The wizard only asks about the common fields (provider/model/base_url/
# api_key_env/thinking/jobs), then tells you which env var to set, e.g.:
export ANTHROPIC_AUTH_TOKEN="sk-ant-..."   # when provider = "anthropic"
# export DEEPSEEK_API_KEY="sk-..."        # when provider = "openai" (DeepSeek native API)
```

### Build

```bash
cd epub-reader
cargo build --release
```

## Quick Start

### 1. Translate a single EPUB

```bash
cargo run --release -- ./books/vol1.epub
```

### 2. Translate a whole directory

```bash
cargo run --release -- ./books ./output
```

### 3. Process Markdown

```bash
cargo run --release -- ./notes/chapter01.md
```

### 4. Process TXT

```bash
cargo run --release -- ./draft.txt
```

### 5. Force TXT line-by-line paragraphs

Useful for poetry, dialogue scripts, or OCR-style short lines:

```bash
cargo run --release -- --txt-hard-linebreaks ./draft.txt
```

### 6. Control concurrency and request pacing

```bash
cargo run --release -- --jobs 3 --request-delay-ms 250 ./books
```

Notes:

- `--jobs` controls concurrent batch requests, not concurrent single paragraphs; you can also set a default `jobs` value in the LLM config
- Concurrency affects throughput only, not the source context attached to each batch; context is always sliced from fixed source positions
- Each batch keeps contiguous paragraphs and tries to stay within `7000` effective characters
- `--context-paragraphs` defaults to the previous `10` source paragraphs; set it to `0` to disable context

### 7. Count text before sending

No API calls and no HTML output. Count only the source text that would enter translation requests:

```bash
cargo run --release -- --count ./books
```

The count includes:

- translatable paragraph count
- effective non-space character count
- effective Chinese character count
- English word count
- request batch count
- estimated input tokens after current batching and prompt wrapping

Code blocks, navigation pages, and text filtered by parsing rules are excluded.

### 8. Rebuild HTML offline

No API calls. Recreate HTML only from existing `*_state.json` files:

```bash
cargo run --release -- --rebuild ./books ./output
```

> `--rebuild` must use the same input source and output directory as the original run so the matching state files can be found.

## CLI Usage

```text
Usage: epub-reader [OPTIONS] <INPUT> [OUTPUT]

Arguments:
  <INPUT>   Input file or directory (.epub/.md/.markdown/.txt)
  [OUTPUT]  Output directory for HTML and state files [default: output]

Options:
      --rebuild
          Rebuild HTML from existing state files without API calls
      --count
          Count translatable source text and exit without API calls or output files
  -v, --verbose
          Print full LLM request and response bodies to stderr
      --jobs <JOBS>
          Maximum number of concurrent translation requests [default: 2]
      --request-delay-ms <REQUEST_DELAY_MS>
          Delay in milliseconds before launching each translation request [default: 0]
      --context-paragraphs <CONTEXT_PARAGRAPHS>
          Number of preceding source paragraphs to include as context for each translation request [default: 10]
      --min-paragraph-chars <MIN_PARAGRAPH_CHARS>
          Minimum characters required for a text block without sentence punctuation [default: 2]
      --title-max-words <TITLE_MAX_WORDS>
          Maximum words to treat a short line as a book title candidate [default: 12]
      --heading-max-words <HEADING_MAX_WORDS>
          Maximum words to treat an uppercase short line as a heading [default: 8]
      --txt-hard-linebreaks
          In .txt files, treat each non-empty line as its own paragraph
      --txt-no-sentence-split
          In .txt files, do not start a new paragraph after sentence-ending punctuation
  -h, --help
          Print help
  -V, --version
          Print version
```

## Supported Input Formats

### EPUB

- Reads content in spine order
- Prefers extracting `p`, `blockquote`, and `li` blocks
- Preserves `pre` code blocks and renders them as read-only highlighted code in HTML
- Falls back to `div` extraction when the document structure is unusual
- Filters some TOC, page-number, and navigation-like pages

### Markdown

- Reads `title` from YAML frontmatter when present
- If there is no frontmatter title, the first suitable `# H1` can become the book title
- `H1-H3` headings are treated as chapter candidates
- Normal paragraphs and list items become translatable text blocks
- Fenced code blocks are preserved in the output HTML and skipped by the translation pipeline

### TXT

- Blank lines and scene breaks create paragraph boundaries
- Tries to recognize headings such as `Chapter 1`, `第十二章`, and `PROLOGUE`
- By default, splits on sentence endings and indented lines
- You can adjust this with `--txt-hard-linebreaks` and `--txt-no-sentence-split`

## Common Use Cases

### Light novels / web novels in EPUB

```bash
cargo run --release -- --jobs 3 ./novels
```

### Markdown notes from Obsidian / Typora

```bash
cargo run --release -- ./notes/book-summary.md
```

### OCR-exported plain text

```bash
cargo run --release -- --txt-hard-linebreaks --min-paragraph-chars 1 ./ocr.txt
```

### Continue a partially finished run

```bash
cargo run --release -- ./books ./output
```

Run the same command again. The program reads `*_state.json` and only requests missing paragraphs.

## Output Files

The default output directory is `./output`.

```text
output/
├── book-slug.html
├── book-slug_state.json
├── another-book.html
└── another-book_state.json
```

- `*.html`
  Final reading file
- `*_state.json`
  Resume state file containing the AI responses for completed paragraphs

> Do not delete `*_state.json` unless you intentionally want to restart from scratch.

The generated HTML also includes reading helpers:

- A chapter drawer in the top-right corner for fast navigation in long books
- A floating location badge showing the current chapter and paragraph index
- Reading position stored as `para_id + in-paragraph offset`, instead of only a coarse scroll percentage
- Persistent open/closed state for translation / vocabulary / chunk sections
- A progress bar based on current paragraph index, so expanding details does not distort the percentage
- Code blocks rendered with an embedded Catppuccin Mocha syntax-highlighting theme

### Reader Theme Preset (Rare Gold + Purple)

The reader no longer uses a bright blue focus color. The current UI theme uses a darker purple-and-gold palette instead: the page stays low-saturation and dark, while the current paragraph, active TOC item, top-right navigator button, and progress bar all use a Diablo-like rare / unique accent. Code highlighting stays on Catppuccin Mocha.

If you want to reuse the same visual system, these are the core CSS tokens:

```css
:root {
  --bg: #1a1b26;
  --surface: #1f2335;
  --border: #3b4168;
  --text: #c0caf5;
  --text-dim: #565f89;

  --accent: #d6b36a;
  --accent-bright: #f0d08c;
  --accent-border: rgba(214, 179, 106, 0.28);
  --focus-rare: #d9c0ff;
  --focus-gear: rgba(168, 117, 255, 0.16);
  --gear-gold: #f1e6cb;

  --purple: #a875ff;
  --rare: #8a52db;
  --rare-soft: rgba(138, 82, 219, 0.18);
  --rare-deep: rgba(72, 34, 104, 0.74);
}
```

General usage notes:

- Buttons and badges: dark purple gradient background with muted gold borders
- Focused English text: `--focus-rare`, the pale purple rare-item text color now used for the active paragraph
- Focused block glow: `--focus-gear`, the purple-and-dark-gold outer glow around the active paragraph block
- Left focus bar: `--gear-gold`, the `#f1e6cb` metallic equipment color
- Current paragraph and active chapter: purple-and-gold glow with a gold edge
- `:focus-visible`: remove the browser's default blue ring and replace it with a thin gold outline plus a purple outer halo
- Progress bar: deep purple into bright purple, ending in muted gold
- Code blocks: keep Catppuccin Mocha separate from the outer reader chrome

The current implementation lives in [src/html_gen.rs](src/html_gen.rs).

## Batching Strategy

The translation stage does not send one paragraph per request by default. It sends small batches of contiguous paragraphs:

- Each request sends an `items` array, where every item includes `id` and `text`
- Each request sends `book.title` for book/volume-specific terminology and tone
- Each request also sends a `context` array by default with the previous `10` source paragraphs for reference only
- Claude must return an `items` array with the same `id` values
- The program validates, reorders, and writes results back by `id`

Current defaults:

- Target batch size: about `5000` effective characters
- Hard per-batch cap: `7000` effective characters
- Maximum per batch: `10` paragraphs
- Single paragraph over `2800` effective characters: sent alone
- Context window: previous `10` source paragraphs, configurable with `--context-paragraphs N`
- Runtime output shows queued batch chars and estimated input tokens including the prompt / `book` / `context` / `items` wrapper
- Batch failure: automatically falls back to single-paragraph requests

This keeps the system prompt cost lower, preserves local reading context, and still lets HTML / state updates stay deterministic.

## Resume / Restart

### Continue from where you left off

Just rerun the same command:

```bash
cargo run --release -- ./books ./output
```

### Rebuild HTML without calling the API

```bash
cargo run --release -- --rebuild ./books ./output
```

This is also the easiest way to refresh previously processed books after the HTML reader UI changes.

### Start over completely

Delete the matching:

- `output/<slug>.html`
- `output/<slug>_state.json`

Then run the command again.

## How It Works

The core idea is not “match by position”, but “match by paragraph ID”.

```text
input file
  └─→ parse_*()
        └─→ Book / Chapter / Paragraph(id, text)
                      │
                      ├─→ html_gen: build paragraph skeleton
                      ├─→ pending: paragraphs that still need LLM calls
                      └─→ state.json: para_id -> LlmResponse
```

Current pipeline:

1. Parse the input file into a unified `Book` structure
2. Generate translatable paragraph skeletons and preserve code blocks as read-only highlighted modules
3. Group contiguous paragraphs into `items[{id, text}]` batches and send them to Claude with bounded concurrency
4. Validate `items[{id, translation, vocabulary, chunks}]` by `para_id`, then patch HTML
5. Atomically write HTML first, then write `*_state.json`
6. In the browser, persist reading position and open detail sections by paragraph anchor

This gives you:

- No paragraph misalignment even when requests finish out of order
- No paragraph misalignment even when items inside a batch come back out of order
- Safe crash behavior, where the worst case is usually redoing one paragraph
- Automatic fallback to single-paragraph retries when a batch fails
- Full `--rebuild` support without any API call
- Better long-form navigation with chapter jumping and paragraph-level resume
- Code samples and terminal snippets remain visible without being mistakenly translated

## Project Structure

```text
src/
├── main.rs            # CLI args, main flow, concurrent translation scheduling
├── parser.rs          # Input format dispatch
├── parse_utils.rs     # Shared segmentation rules, heading detection, BookBuilder
├── epub_parser.rs     # EPUB parsing
├── markdown_parser.rs # Markdown parsing
├── text_parser.rs     # TXT parsing
├── html_gen.rs        # HTML generation and paragraph patching
├── config/            # LLM config: path resolution, schema, load/merge, setup wizard
├── llm_client.rs      # Anthropic / OpenAI-compatible client
├── state.rs           # state.json read/write
├── fs_utils.rs        # Atomic file writing
├── ui.rs              # Terminal presentation
└── types.rs           # Book / Paragraph / LlmResponse structures
```

## LLM Configuration

The config file does not live in the project directory; it lives in the
platform-standard config directory instead (override with `--llm-config`):

- Linux: `$XDG_CONFIG_HOME/epub-reader/llm.toml` (falls back to `~/.config/epub-reader/llm.toml`)
- macOS: `~/Library/Application Support/epub-reader/llm.toml`
- Windows: `%APPDATA%\epub-reader\llm.toml`

The project's own `llm.toml` is just a commented template/example; the running program never reads it.

Easiest way to configure: run the interactive wizard, which only asks about the common fields and writes the result immediately:

```bash
cargo run --release -- --setup
```

The file can also be hand-written:

```toml
[llm]
provider = "anthropic"        # "anthropic" or "openai"
model = "deepseek-v4-flash"
base_url = "https://api.anthropic.com"
thinking = false              # off by default; both DeepSeek and Claude support enabling it
# thinking_budget_tokens = 4096
# api_key_env = "ANTHROPIC_AUTH_TOKEN"
# max_output_tokens = 8192
# request_timeout_secs = 180
jobs = 2
```

- `provider = "anthropic"`: speaks the Anthropic Messages API (`/v1/messages`), compatible with the official endpoint and relay gateways
- `provider = "openai"`: speaks the OpenAI Chat Completions format (`/chat/completions`), compatible with DeepSeek's native API
- The API key always comes from an environment variable, named by `api_key_env` (defaults to `ANTHROPIC_AUTH_TOKEN` for `anthropic`, `DEEPSEEK_API_KEY` for `openai`); it is never written to the config file
- Precedence: CLI overrides > config file > built-in defaults. CLI overrides: `--llm-provider`, `--llm-model`, `--llm-base-url`, `--llm-thinking` / `--llm-no-thinking`, `--jobs`
- `thinking` is off by default; `--llm-thinking` and `--llm-no-thinking` cannot be used together
- With no config file present, the program falls back to CLI flags plus built-in defaults; run `--setup` to create one interactively

## Notes

- An API key is only required in translation mode, via the environment variable named by the config file's `api_key_env`; `--rebuild` does not need one
- If you modify the source input after starting a run, paragraph IDs may change and old state may no longer align perfectly
- `--jobs` can be set very high without changing translation context, but whether that helps depends on your provider's concurrency / RPM / TPM limits and on how many batches the current job actually has
- For messy TXT input, try:
  - `--txt-hard-linebreaks`
  - `--min-paragraph-chars 1`
  - `--txt-no-sentence-split`

## Development

```bash
cargo fmt
cargo check
cargo test
```

To inspect the live CLI help:

```bash
cargo run -- --help
```
