use crate::types::{Book, LlmResponse, Paragraph, ParagraphKind};
use html_escape::encode_text;
use std::fmt::Write as FmtWrite;
use std::io::Cursor;
use std::sync::OnceLock;
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::highlighted_html_for_string;
use syntect::parsing::{SyntaxReference, SyntaxSet};

struct CodeHighlightAssets {
    syntax_set: SyntaxSet,
    theme: Theme,
}

/// Generate a full HTML page for the given book.
/// Each paragraph gets placeholder `<details>` sections.
pub fn generate_html(book: &Book) -> String {
    let mut body = String::new();
    let toc = render_toc(book);

    for chapter in &book.chapters {
        let ch_title = chapter
            .title
            .clone()
            .unwrap_or_else(|| format!("Chapter {}", chapter.index + 1));

        writeln!(
            body,
            r#"<section class="chapter" id="ch{:03}">"#,
            chapter.index
        )
        .unwrap();
        writeln!(
            body,
            r#"  <h2 class="chapter-title">{}</h2>"#,
            encode_text(&ch_title)
        )
        .unwrap();

        for para in &chapter.paragraphs {
            body.push_str(&render_chapter_block(para, None));
        }

        body.push_str("</section>\n");
    }

    HTML_TEMPLATE
        .replace("{{TITLE}}", &encode_text(&book.title))
        .replace("{{SLUG}}", &book.slug)
        .replace("{{TOC}}", &toc)
        .replace("{{BODY}}", &body)
}

fn render_chapter_block(para: &Paragraph, resp: Option<&LlmResponse>) -> String {
    match &para.kind {
        ParagraphKind::Text => render_para_block(para, resp),
        ParagraphKind::CodeBlock { language } => render_code_block(para, language.as_deref()),
    }
}

/// Render a single paragraph block. If `resp` is Some, fills in the LLM content.
pub fn render_para_block(para: &Paragraph, resp: Option<&LlmResponse>) -> String {
    let status = if resp.is_some() { "done" } else { "pending" };
    let original = encode_text(&para.text);

    let translation_html = match resp {
        Some(r) => format!("<p>{}</p>", encode_text(&r.translation)),
        None => "<!-- FILL:translation -->".to_string(),
    };

    let vocab_html = match resp {
        Some(r) => render_vocab(&r.vocabulary),
        None => "<!-- FILL:vocab -->".to_string(),
    };

    let chunks_html = match resp {
        Some(r) => render_chunks(&r.chunks),
        None => "<!-- FILL:chunks -->".to_string(),
    };

    format!(
        r#"<div class="para-block" id="{id}" data-status="{status}">
  <p class="original-text">{original}</p>
  <details class="ai-section translation-section" data-detail-key="{id}:translation">
    <summary><span class="section-icon">🈳</span> 译文</summary>
    <div class="ai-content">{translation_html}</div>
  </details>
  <details class="ai-section vocab-section" data-detail-key="{id}:vocab">
    <summary><span class="section-icon">📚</span> 词汇 (IELTS 6.5+)</summary>
    <div class="ai-content">{vocab_html}</div>
  </details>
  <details class="ai-section chunk-section" data-detail-key="{id}:chunks">
    <summary><span class="section-icon">🔗</span> 常用短语 / Chunks</summary>
    <div class="ai-content">{chunks_html}</div>
  </details>
</div>
"#,
        id = para.id,
        status = status,
        original = original,
        translation_html = translation_html,
        vocab_html = vocab_html,
        chunks_html = chunks_html,
    )
}

fn render_code_block(para: &Paragraph, language: Option<&str>) -> String {
    let label = language
        .map(str::trim)
        .filter(|lang| !lang.is_empty())
        .unwrap_or("code");
    let highlighted = highlight_code_html(&para.text, language);

    format!(
        r#"<figure class="code-block" id="{id}">
  <figcaption class="code-block-label">{label}</figcaption>
  <div class="code-block-html">{highlighted}</div>
</figure>
"#,
        id = para.id,
        label = encode_text(label),
        highlighted = highlighted,
    )
}

fn highlight_code_html(code: &str, language: Option<&str>) -> String {
    let assets = code_highlight_assets();
    let syntax = pick_syntax(&assets.syntax_set, language, code);

    highlighted_html_for_string(code, &assets.syntax_set, syntax, &assets.theme).unwrap_or_else(
        |_| {
            format!(
                "<pre><code>{}</code></pre>",
                encode_text(code.trim_matches('\n'))
            )
        },
    )
}

fn pick_syntax<'a>(
    syntax_set: &'a SyntaxSet,
    language: Option<&str>,
    code: &str,
) -> &'a SyntaxReference {
    language
        .and_then(normalized_language_token)
        .and_then(|lang| syntax_set.find_syntax_by_token(&lang))
        .or_else(|| syntax_set.find_syntax_by_first_line(code))
        .unwrap_or_else(|| syntax_set.find_syntax_plain_text())
}

fn normalized_language_token(language: &str) -> Option<String> {
    let token = language.trim().to_ascii_lowercase();
    if token.is_empty() {
        return None;
    }

    let normalized = match token.as_str() {
        "c++" => "cpp",
        "c#" => "cs",
        "js" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "rb" => "ruby",
        "rs" => "rust",
        "sh" => "bash",
        "shell" => "bash",
        "zsh" => "bash",
        "ps1" => "powershell",
        "yml" => "yaml",
        "md" => "markdown",
        other => other,
    };

    Some(normalized.to_string())
}

fn code_highlight_assets() -> &'static CodeHighlightAssets {
    static ASSETS: OnceLock<CodeHighlightAssets> = OnceLock::new();
    ASSETS.get_or_init(|| {
        let syntax_set = SyntaxSet::load_defaults_newlines();
        let mut reader = Cursor::new(include_bytes!("../assets/catppuccin-mocha.tmTheme"));
        let theme =
            ThemeSet::load_from_reader(&mut reader).expect("failed to load Catppuccin Mocha theme");

        CodeHighlightAssets { syntax_set, theme }
    })
}

fn render_toc(book: &Book) -> String {
    let mut s = String::from(r#"<nav class="toc-nav" aria-label="Chapter navigation">"#);

    for chapter in &book.chapters {
        let title = chapter
            .title
            .clone()
            .unwrap_or_else(|| format!("Chapter {}", chapter.index + 1));
        let chapter_id = format!("ch{:03}", chapter.index);

        s.push_str(&format!(
            r##"<a class="toc-link" href="#{id}" data-chapter-id="{id}"><span class="toc-link-index">{index:02}</span><span class="toc-link-title">{title}</span><span class="toc-link-meta">{para_count} p</span></a>"##,
            id = chapter_id,
            index = chapter.index + 1,
            title = encode_text(&title),
            para_count = chapter.paragraphs.iter().filter(|p| p.is_translatable()).count(),
        ));
    }

    s.push_str("</nav>");
    s
}

fn render_vocab(entries: &[crate::types::VocabEntry]) -> String {
    if entries.is_empty() {
        return "<p class=\"empty\">—</p>".to_string();
    }
    let mut s = String::from(
        r#"<div class="vocab-scroll"><table class="vocab-table"><thead><tr><th>单词</th><th>音标</th><th>词性</th><th>释义</th><th>例句</th></tr></thead><tbody>"#,
    );
    for e in entries {
        s.push_str(&format!(
            "<tr><td class=\"word\">{}</td><td class=\"ipa\">{}</td><td class=\"pos\">{}</td><td class=\"meaning\">{}</td><td class=\"example\"><em>{}</em></td></tr>",
            encode_text(&e.word),
            encode_text(&e.ipa),
            encode_text(&e.pos),
            encode_text(&e.cn),
            encode_text(&e.example),
        ));
    }
    s.push_str("</tbody></table></div>");
    s
}

fn render_chunks(entries: &[crate::types::ChunkEntry]) -> String {
    if entries.is_empty() {
        return "<p class=\"empty\">—</p>".to_string();
    }
    let mut s = String::from(r#"<ul class="chunk-list">"#);
    for e in entries {
        s.push_str(&format!(
            r#"<li><span class="chunk-phrase">{}</span> <span class="chunk-cn">（{}）</span><br><em class="chunk-example">{}</em></li>"#,
            encode_text(&e.chunk),
            encode_text(&e.cn),
            encode_text(&e.example),
        ));
    }
    s.push_str("</ul>");
    s
}

/// Update a single paragraph block inside an existing HTML string in-place.
/// Finds the `<div class="para-block" id="{id}" ...>` block and replaces it.
pub fn patch_html(html: &str, para: &Paragraph, resp: &LlmResponse) -> String {
    if !para.is_translatable() {
        return html.to_string();
    }

    let new_block = render_para_block(para, Some(resp));

    // Find the start tag by id attribute
    let id_marker = format!("id=\"{}\"", para.id);
    let start = match html.find(&id_marker) {
        Some(pos) => {
            // Walk back to find the `<div`
            match html[..pos].rfind("<div") {
                Some(p) => p,
                None => return html.to_string(),
            }
        }
        None => return html.to_string(),
    };

    // Find the matching closing `</div>` — count nesting depth.
    // Operate on raw bytes so emoji/multibyte chars never cause a slice-boundary panic.
    let after_start = &html[start..];
    let bytes = after_start.as_bytes();
    let mut depth = 0usize;
    let mut end = start;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"<div") {
            depth += 1;
            i += 4;
        } else if bytes[i..].starts_with(b"</div>") {
            if depth == 1 {
                end = start + i + 6; // include `</div>`
                break;
            }
            depth -= 1;
            i += 6;
        } else {
            i += 1;
        }
    }

    if end == start {
        return html.to_string();
    }

    format!("{}{}{}", &html[..start], new_block, &html[end..])
}

// ─── HTML Template ────────────────────────────────────────────────────────────

const HTML_TEMPLATE: &str = r#"<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="UTF-8" />
  <meta name="viewport" content="width=device-width, initial-scale=1.0" />
  <title>{{TITLE}}</title>
  <style>
    /* ── Reset & Base ───────────────────────────────────────────── */
    *, *::before, *::after { box-sizing: border-box; margin: 0; padding: 0; }
    :root {
      --bg:        #1a1b26;
      --bg2:       #24283b;
      --bg3:       #2d3149;
      --surface:   #1f2335;
      --border:    #3b4168;
      --text:      #c0caf5;
      --text-dim:  #565f89;
      --accent:    #d6b36a;
      --accent-bright: #f0d08c;
      --accent-soft: rgba(214, 179, 106, .16);
      --accent-border: rgba(214, 179, 106, .28);
      --green:     #9ece6a;
      --yellow:    #e0af68;
      --red:       #f7768e;
      --focus-rare: #d9c0ff;
      --focus-gear: rgba(168, 117, 255, .16);
      --gear-gold: #f1e6cb;
      --rare-item-bg: #5f4716;
      --cyan:      var(--focus-rare);
      --purple:    #a875ff;
      --rare:      #8a52db;
      --rare-soft: rgba(138, 82, 219, .18);
      --rare-deep: rgba(72, 34, 104, .74);
      --orange:    #ff9e64;
      --chapter-title-color: #c89cff;
      --theme-toggle-border: rgba(217, 192, 255, .28);
      --theme-toggle-bg: rgba(28, 33, 50, .88);
      --theme-toggle-fg: var(--focus-rare);
      --theme-toggle-focus-border: rgba(217, 192, 255, .45);
      --theme-toggle-focus-ring: rgba(217, 192, 255, .16);
      --toc-toggle-border: rgba(217, 192, 255, .28);
      --toc-toggle-bg: rgba(28, 33, 50, .88);
      --toc-toggle-fg: var(--focus-rare);
      --toc-toggle-focus-border: rgba(217, 192, 255, .45);
      --toc-toggle-focus-ring: rgba(217, 192, 255, .16);
      --sidebar-panel-bg: linear-gradient(180deg, rgba(46, 32, 67, .98) 0%, rgba(26, 27, 38, .98) 100%);
      --sidebar-panel-border: rgba(217, 192, 255, .12);
      --sidebar-head-border: rgba(217, 192, 255, .12);
      --sidebar-book-title-color: var(--chapter-title-color);
      --sidebar-link-hover-bg: rgba(168, 117, 255, .08);
      --sidebar-link-hover-border: rgba(217, 192, 255, .14);
      --sidebar-link-current-bg: rgba(168, 117, 255, .12);
      --sidebar-link-current-border: rgba(217, 192, 255, .26);
      --sidebar-link-current-shadow: rgba(168, 117, 255, .08);
      --sidebar-link-focus-border: rgba(217, 192, 255, .3);
      --sidebar-link-focus-ring: rgba(217, 192, 255, .14);
      --translation-summary-bg: #201d30;
      --translation-summary-fg: var(--cyan);
      --vocab-summary-bg: #1d2940;
      --vocab-summary-fg: var(--gear-gold);
      --chunk-summary-bg: #1e2a20;
      --chunk-summary-fg: var(--green);
      --progress-start: var(--focus-rare);
      --progress-mid: var(--gear-gold);
      --progress-end: #fff1b8;
      --progress-glow-a: rgba(217, 192, 255, .24);
      --progress-glow-b: rgba(255, 232, 170, .24);
      --progress-sheen: rgba(255,255,255,.22);
      --progress-dot: #fff1b8;
      --progress-dot-glow-a: #fff1b8;
      --progress-dot-glow-b: rgba(255, 223, 122, .78);
      --progress-dot-glow-c: rgba(255, 196, 72, .42);
      --radius:    8px;
      font-size:   17px;
    }
    body[data-theme="legacy"] {
      --accent: #7aa2f7;
      --accent-bright: #7dcfff;
      --accent-soft: rgba(122, 162, 247, .16);
      --accent-border: rgba(122, 162, 247, .28);
      --focus-rare: #7dcfff;
      --focus-gear: rgba(122, 162, 247, .16);
      --gear-gold: #7aa2f7;
      --rare-item-bg: #201e30;
      --cyan: #7dcfff;
      --purple: #bb9af7;
      --chapter-title-color: #bb9af7;
      --translation-summary-bg: #1e2940;
      --translation-summary-fg: #7dcfff;
      --vocab-summary-bg: #201d30;
      --vocab-summary-fg: #bb9af7;
      --chunk-summary-bg: #1e2a20;
      --chunk-summary-fg: #9ece6a;
      --theme-toggle-border: rgba(125, 207, 255, .28);
      --theme-toggle-bg: rgba(28, 33, 50, .88);
      --theme-toggle-fg: #7dcfff;
      --theme-toggle-focus-border: rgba(125, 207, 255, .45);
      --theme-toggle-focus-ring: rgba(125, 207, 255, .16);
      --toc-toggle-border: rgba(125, 207, 255, .28);
      --toc-toggle-bg: rgba(28, 33, 50, .88);
      --toc-toggle-fg: #7dcfff;
      --toc-toggle-focus-border: rgba(125, 207, 255, .45);
      --toc-toggle-focus-ring: rgba(125, 207, 255, .16);
      --sidebar-panel-bg: linear-gradient(180deg, rgba(34, 40, 61, .98) 0%, rgba(23, 28, 44, .98) 100%);
      --sidebar-panel-border: rgba(125, 207, 255, .12);
      --sidebar-head-border: rgba(125, 207, 255, .12);
      --sidebar-book-title-color: #7aa2f7;
      --sidebar-link-hover-bg: rgba(122, 162, 247, .08);
      --sidebar-link-hover-border: rgba(122, 162, 247, .14);
      --sidebar-link-current-bg: rgba(122, 162, 247, .12);
      --sidebar-link-current-border: rgba(122, 162, 247, .26);
      --sidebar-link-current-shadow: rgba(122, 162, 247, .08);
      --sidebar-link-focus-border: rgba(122, 162, 247, .3);
      --sidebar-link-focus-ring: rgba(122, 162, 247, .14);
      --progress-start: #7dcfff;
      --progress-mid: #f1e6cb;
      --progress-end: #fff1b8;
      --progress-glow-a: rgba(125, 207, 255, .24);
      --progress-glow-b: rgba(255, 232, 170, .24);
      --progress-dot: #fff1b8;
      --progress-dot-glow-a: #fff1b8;
      --progress-dot-glow-b: rgba(255, 223, 122, .78);
      --progress-dot-glow-c: rgba(255, 196, 72, .42);
    }
    body {
      background: var(--bg);
      color: var(--text);
      font-family: 'Georgia', 'Noto Serif SC', serif;
      line-height: 1.85;
      max-width: 860px;
      margin: 0 auto;
      padding: 2rem 1.5rem 6rem;
    }
    body.toc-open { overflow: hidden; }
    a { color: var(--accent); }

    /* ── Floating UI ────────────────────────────────────────────── */
    #theme-toggle,
    #toc-toggle {
      position: fixed;
      top: 1rem;
      z-index: 140;
      border: 1px solid transparent;
      font: 600 .82rem/1 'Segoe UI', system-ui, sans-serif;
      letter-spacing: .04em;
      border-radius: 999px;
      padding: .58rem .9rem;
      cursor: pointer;
      backdrop-filter: blur(12px);
      box-shadow: 0 12px 30px rgba(0,0,0,.26);
    }
    #theme-toggle {
      left: 1rem;
      border-color: var(--theme-toggle-border);
      background: var(--theme-toggle-bg);
      color: var(--theme-toggle-fg);
    }
    #toc-toggle {
      right: 1rem;
      border-color: var(--toc-toggle-border);
      background: var(--toc-toggle-bg);
      color: var(--toc-toggle-fg);
    }
    #theme-toggle:focus-visible {
      outline: none;
      box-shadow:
        0 0 0 1px var(--theme-toggle-focus-border),
        0 0 0 4px var(--theme-toggle-focus-ring),
        0 12px 30px rgba(0,0,0,.26);
    }
    #toc-toggle:focus-visible {
      outline: none;
      box-shadow:
        0 0 0 1px var(--toc-toggle-focus-border),
        0 0 0 4px var(--toc-toggle-focus-ring),
        0 12px 30px rgba(0,0,0,.26);
    }
    #toc-backdrop {
      position: fixed;
      inset: 0;
      z-index: 118;
      background: rgba(7, 10, 20, .48);
      opacity: 0;
      pointer-events: none;
      transition: opacity .2s ease;
    }
    body.toc-open #toc-backdrop {
      opacity: 1;
      pointer-events: auto;
    }
    #toc-panel {
      position: fixed;
      top: 0;
      right: 0;
      z-index: 130;
      width: min(24rem, 92vw);
      height: 100vh;
      padding: 1.25rem 1rem 1.4rem;
      background: var(--sidebar-panel-bg);
      border-left: 1px solid var(--sidebar-panel-border);
      box-shadow: -24px 0 48px rgba(0,0,0,.34);
      overflow-y: auto;
      transform: translateX(104%);
      transition: transform .25s ease;
      backdrop-filter: blur(18px);
    }
    body.toc-open #toc-panel { transform: translateX(0); }
    .toc-head {
      margin-bottom: 1rem;
      padding-bottom: .9rem;
      border-bottom: 1px solid var(--sidebar-head-border);
    }
    .toc-kicker {
      font: 700 .68rem/1 'Segoe UI', system-ui, sans-serif;
      letter-spacing: .18em;
      text-transform: uppercase;
      color: var(--text-dim);
      margin-bottom: .45rem;
    }
    .toc-book-title {
      color: var(--sidebar-book-title-color);
      font-size: 1.02rem;
      line-height: 1.45;
      margin-bottom: .55rem;
      word-break: break-word;
    }
    #toc-loc-inline {
      color: var(--text-dim);
      font: 600 .76rem/1.35 'Segoe UI', system-ui, sans-serif;
      min-height: 1.2rem;
    }
    .toc-nav {
      display: flex;
      flex-direction: column;
      gap: .45rem;
      padding-bottom: 4rem;
    }
    .toc-link {
      display: grid;
      grid-template-columns: auto 1fr auto;
      align-items: baseline;
      gap: .7rem;
      padding: .68rem .8rem;
      border-radius: 12px;
      text-decoration: none;
      color: var(--text);
      background: rgba(255,255,255,.02);
      border: 1px solid transparent;
      transition: transform .16s ease, border-color .16s ease, background .16s ease;
    }
    .toc-link:hover {
      transform: translateX(-2px);
      background: var(--sidebar-link-hover-bg);
      border-color: var(--sidebar-link-hover-border);
    }
    .toc-link.is-current {
      background: var(--sidebar-link-current-bg);
      border-color: var(--sidebar-link-current-border);
      box-shadow: inset 0 0 0 1px var(--sidebar-link-current-shadow);
    }
    .toc-link:focus-visible {
      outline: none;
      border-color: var(--sidebar-link-focus-border);
      box-shadow:
        inset 0 0 0 1px var(--sidebar-link-current-shadow),
        0 0 0 3px var(--sidebar-link-focus-ring);
    }
    .toc-link-index {
      color: var(--text-dim);
      font: 700 .72rem/1 'Segoe UI', system-ui, sans-serif;
      letter-spacing: .08em;
      min-width: 2ch;
    }
    .toc-link-title {
      color: var(--text);
      font: 600 .86rem/1.35 'Segoe UI', system-ui, sans-serif;
    }
    .toc-link-meta {
      color: var(--text-dim);
      font: 600 .72rem/1 'Segoe UI', system-ui, sans-serif;
    }

    /* ── Progress bar (bottom) ──────────────────────────────────── */
    #progress-bar-wrap {
      position: fixed; bottom: 1.2rem; left: 50%;
      transform: translateX(-50%);
      width: min(1000px, 90%); height: 5px;
      background: rgba(255,255,255,.08);
      border-radius: 9999px;
      z-index: 100;
      box-shadow: 0 0 12px rgba(0,0,0,.5);
    }
    #progress-pct {
      position: fixed; bottom: 1.9rem; right: calc(50% - min(500px, 45%) + 0px);
      font-size: .7rem; font-family: monospace;
      color: rgba(255,255,255,.35);
      z-index: 100;
      pointer-events: none;
      user-select: none;
      white-space: nowrap;
    }
    #progress-bar {
      height: 100%;
      width: 0%;
      border-radius: 9999px;
      transition: width .25s ease;
      background: linear-gradient(90deg, var(--progress-start) 0%, var(--progress-mid) 50%, var(--progress-end) 90%, var(--progress-end) 100%);
      box-shadow:
        0 0 12px var(--progress-glow-a),
        0 0 32px var(--progress-glow-b);
      position: relative;
      overflow: hidden;
    }
    #progress-bar::before {
      content: '';
      position: absolute;
      inset: 0;
      background: linear-gradient(180deg, var(--progress-sheen) 0%, rgba(255,255,255,0) 74%);
      pointer-events: none;
    }
    #progress-bar::after {
      content: '';
      position: absolute;
      right: -1px; top: 50%;
      transform: translateY(-50%);
      width: 5px; height: 5px;
      border-radius: 50%;
      background: var(--progress-dot);
      box-shadow: 0 0 10px 3px var(--progress-dot-glow-a), 0 0 24px 7px var(--progress-dot-glow-b), 0 0 40px 10px var(--progress-dot-glow-c);
      opacity: 1;
      transition: opacity .25s;
    }
    #progress-bar[style*="width: 0"]::after { opacity: 0; }

    /* ── Chapter ───────────────────────────────────────────────── */
    .chapter {
      margin-bottom: 4rem;
      scroll-margin-top: 1rem;
    }
    .chapter-title {
      font-size: 1.6rem; color: var(--chapter-title-color);
      border-bottom: 2px solid var(--border);
      padding-bottom: .4rem; margin-bottom: 2rem;
    }

    /* ── Paragraph block ───────────────────────────────────────── */
    .para-block {
      margin-bottom: 2rem;
      border-left: 3px solid var(--border);
      border-radius: 0 4px 4px 0;
      padding: .7rem 1rem 19px 1rem;
      transition: border-color .2s;
      scroll-margin-top: 1rem;
    }
    .para-block[data-status="done"] { border-left-color: var(--green); }
    .para-block[data-status="pending"] { border-left-color: var(--border); }
    .para-block.is-current {
      border-left-color: var(--gear-gold);
      background: transparent;
    }

    .original-text {
      font-size: 1rem;
      color: var(--text);
      margin-bottom: .6rem;
      text-align: justify;
    }
    .para-block.is-current .original-text {
      color: var(--focus-rare);
      background: linear-gradient(90deg, var(--text) 0%, var(--text) 199%);
      -webkit-background-clip: text;
      background-clip: text;
      -webkit-text-fill-color: transparent;
      text-shadow:
        0 0 10px rgba(214, 179, 106, .08),
        0 0 18px rgba(138, 82, 219, .05);
    }
    /* ── Collapsible AI sections ───────────────────────────────── */
    .ai-section {
      margin-top: .35rem;
      border-radius: var(--radius);
      overflow: hidden;
    }
    .ai-section > summary {
      cursor: pointer;
      padding: .3rem .7rem;
      font-size: .82rem;
      font-family: 'Segoe UI', system-ui, sans-serif;
      font-weight: 600;
      letter-spacing: .03em;
      list-style: none;
      display: flex; align-items: center; gap: .4rem;
      user-select: none;
    }
    .ai-section > summary::-webkit-details-marker { display: none; }
    .ai-section > summary::before {
      content: '▶'; font-size: .6rem; transition: transform .15s;
    }
    .ai-section[open] > summary::before { transform: rotate(90deg); }
    .ai-section > summary:focus-visible {
      outline: none;
      box-shadow:
        inset 0 0 0 1px rgba(240, 208, 140, .12),
        0 0 0 3px rgba(138, 82, 219, .16);
    }

    .translation-section > summary { background: var(--translation-summary-bg); color: var(--translation-summary-fg); }
    .vocab-section      > summary { background: var(--vocab-summary-bg); color: var(--vocab-summary-fg); }
    .chunk-section      > summary { background: var(--chunk-summary-bg); color: var(--chunk-summary-fg); }

    .ai-content {
      padding: .7rem 1rem;
      font-size: .9rem;
      font-family: 'Segoe UI', system-ui, sans-serif;
      line-height: 1.7;
      background: var(--surface);
    }

    /* Translation */
    .translation-section .ai-content p { color: var(--cyan); }

    /* Code block */
    .code-block {
      margin: 1.1rem 0 1.8rem;
      border: 1px solid rgba(122, 162, 247, .12);
      border-radius: 14px;
      overflow: hidden;
      background: rgba(16, 19, 31, .92);
      box-shadow: 0 14px 30px rgba(0,0,0,.18);
    }
    .code-block-label {
      padding: .45rem .8rem;
      color: var(--text-dim);
      background: rgba(122, 162, 247, .06);
      border-bottom: 1px solid rgba(122, 162, 247, .08);
      font: 700 .73rem/1 'Segoe UI', system-ui, sans-serif;
      letter-spacing: .08em;
      text-transform: uppercase;
    }
    .code-block-html {
      overflow-x: auto;
    }
    .code-block-html pre {
      overflow-x: auto;
      margin: 0;
      padding: .95rem 1rem 1.05rem;
    }
    .code-block-html code {
      display: block;
      color: #d5defc;
      font: .86rem/1.6 'JetBrains Mono', 'Fira Code', 'SFMono-Regular', Consolas, monospace;
      white-space: pre;
      tab-size: 2;
    }
    .code-block-html pre[style] {
      border-radius: 0;
      margin: 0;
      background: #161821 !important;
    }

    /* Vocab table */
    .vocab-scroll {
      overflow-x: auto;
      -webkit-overflow-scrolling: touch;
    }
    .vocab-table {
      width: 100%; border-collapse: collapse;
      font-size: .82rem;
    }
    .vocab-table th {
      background: var(--bg2); color: var(--text-dim);
      font-weight: 600; text-align: left;
      padding: .3rem .5rem;
      border-bottom: 1px solid var(--border);
    }
    .vocab-table td {
      padding: .3rem .5rem;
      border-bottom: 1px solid var(--bg3);
      vertical-align: top;
    }
    .vocab-table tr:last-child td { border-bottom: none; }
    .vocab-table .word    { color: var(--gear-gold); font-weight: 700; }
    .vocab-table .ipa     { color: var(--text-dim); font-family: monospace; }
    .vocab-table .pos     { color: var(--gear-gold); font-style: italic; }
    .vocab-table .meaning { color: var(--gear-gold); }
    .vocab-table .example { color: var(--text-dim); }

    /* Chunk list */
    .chunk-list { list-style: none; }
    .chunk-list li { margin-bottom: .6rem; }
    .chunk-phrase { color: var(--green); font-weight: 700; }
    .chunk-cn     { color: var(--text-dim); font-size: .82rem; }
    .chunk-example { color: var(--text-dim); font-size: .85rem; }

    .empty { color: var(--text-dim); font-style: italic; }

    /* ── Scrollbar ─────────────────────────────────────────────── */
    ::-webkit-scrollbar { width: 6px; }
    ::-webkit-scrollbar-track { background: var(--bg); }
    ::-webkit-scrollbar-thumb { background: var(--border); border-radius: 3px; }

    /* ── Responsive ────────────────────────────────────────────── */
    @media (max-width: 760px) {
      #toc-panel { width: min(26rem, 96vw); }
      #progress-pct { right: 1rem; bottom: 1.9rem; }
    }
    @media (max-width: 600px) {
      body { font-size: 15px; padding: 1rem .8rem 5.6rem; }
      #theme-toggle { top: .8rem; left: .8rem; }
      #toc-toggle { top: .8rem; right: .8rem; }
      .vocab-table { font-size: .75rem; }
    }
  </style>
</head>
<body>
  <button id="theme-toggle" type="button" aria-pressed="false">Relic</button>
  <button id="toc-toggle" type="button" aria-expanded="false" aria-controls="toc-panel">Chapters</button>
  <div id="toc-backdrop" aria-hidden="true"></div>
  <aside id="toc-panel" aria-hidden="true">
    <div class="toc-head">
      <div class="toc-kicker">Navigator</div>
      <div class="toc-book-title">{{TITLE}}</div>
      <div id="toc-loc-inline"></div>
    </div>
    {{TOC}}
  </aside>

  <div id="progress-bar-wrap"><div id="progress-bar"></div></div>
  <div id="progress-pct">0.00%</div>

  <h1 style="color:var(--accent);margin-bottom:2.5rem;font-size:2rem;">{{TITLE}}</h1>

  {{BODY}}

  <script>
    // Reading state and navigation
    const bar = document.getElementById('progress-bar');
    const pctEl = document.getElementById('progress-pct');
    const themeToggle = document.getElementById('theme-toggle');
    const tocLocInlineEl = document.getElementById('toc-loc-inline');
    const tocToggle = document.getElementById('toc-toggle');
    const tocPanel = document.getElementById('toc-panel');
    const tocBackdrop = document.getElementById('toc-backdrop');
    const tocLinks = Array.from(document.querySelectorAll('.toc-link'));
    const paraBlocks = Array.from(document.querySelectorAll('.para-block'));
    const detailSections = Array.from(document.querySelectorAll('.ai-section[data-detail-key]'));
    const THEME_KEY = 'reader-theme';
    const POSITION_KEY = 'reading-position:{{SLUG}}';
    const DETAILS_KEY = 'open-details:{{SLUG}}';
    const VIEWPORT_ANCHOR_RATIO = 0.5;
    const PROGRESS_MID_POINT = 0.35;
    let currentParaId = null;
    let rafPending = false;
    let canSaveReadingPosition = false;

    function safeParse(raw, fallback) {
      if (!raw) return fallback;
      try {
        return JSON.parse(raw);
      } catch (_) {
        return fallback;
      }
    }

    function saveJson(key, value) {
      try {
        localStorage.setItem(key, JSON.stringify(value));
      } catch (_) {}
    }

    function clamp01(value) {
      return Math.max(0, Math.min(1, value));
    }

    function cssVar(name) {
      return getComputedStyle(document.body).getPropertyValue(name).trim();
    }

    function hexToRgb(hex) {
      const normalized = hex.replace('#', '');
      const full = normalized.length === 3
        ? normalized.split('').map(char => char + char).join('')
        : normalized;

      return {
        r: parseInt(full.slice(0, 2), 16),
        g: parseInt(full.slice(2, 4), 16),
        b: parseInt(full.slice(4, 6), 16),
      };
    }

    function mixHexColor(left, right, t) {
      const ratio = clamp01(t);
      const a = hexToRgb(left);
      const b = hexToRgb(right);
      const r = Math.round(a.r + (b.r - a.r) * ratio);
      const g = Math.round(a.g + (b.g - a.g) * ratio);
      const bVal = Math.round(a.b + (b.b - a.b) * ratio);
      return `rgb(${r}, ${g}, ${bVal})`;
    }

    function progressEndColor(progress01) {
      const startColor = cssVar('--progress-start');
      const midColor = cssVar('--progress-mid');
      const endColor = cssVar('--progress-end');
      if (progress01 <= PROGRESS_MID_POINT) {
        return mixHexColor(startColor, midColor, progress01 / PROGRESS_MID_POINT);
      }
      return mixHexColor(
        midColor,
        endColor,
        (progress01 - PROGRESS_MID_POINT) / (1 - PROGRESS_MID_POINT)
      );
    }

    function updateProgressBarVisual(pct) {
      const progress01 = clamp01(pct / 100);
      const endColor = progressEndColor(progress01);
      bar.style.background = `linear-gradient(90deg, ${cssVar('--progress-start')} 0%, ${endColor} 100%)`;
    }

    function getScrollTop() {
      return window.scrollY || document.documentElement.scrollTop || 0;
    }

    function setTocOpen(open) {
      document.body.classList.toggle('toc-open', open);
      tocToggle.setAttribute('aria-expanded', open ? 'true' : 'false');
      tocPanel.setAttribute('aria-hidden', open ? 'false' : 'true');
    }

    function themeLabel(theme) {
      return theme === 'legacy' ? 'Classic' : 'Relic';
    }

    function applyTheme(theme, persist = true) {
      const normalized = theme === 'legacy' ? 'legacy' : 'current';
      document.body.dataset.theme = normalized;
      themeToggle.textContent = themeLabel(normalized);
      themeToggle.setAttribute('aria-pressed', normalized === 'legacy' ? 'true' : 'false');
      if (persist) {
        try {
          localStorage.setItem(THEME_KEY, normalized);
        } catch (_) {}
      }
      scheduleReadingState();
    }

    function getCurrentParagraph() {
      if (!paraBlocks.length) return null;
      const anchorY = getScrollTop() + window.innerHeight * VIEWPORT_ANCHOR_RATIO;
      let low = 0;
      let high = paraBlocks.length - 1;
      let best = 0;

      while (low <= high) {
        const mid = Math.floor((low + high) / 2);
        if (paraBlocks[mid].offsetTop <= anchorY) {
          best = mid;
          low = mid + 1;
        } else {
          high = mid - 1;
        }
      }

      return paraBlocks[best];
    }

    function updateCurrentHighlight(para) {
      if (currentParaId === para.id) return;
      if (currentParaId) {
        document.getElementById(currentParaId)?.classList.remove('is-current');
      }
      para.classList.add('is-current');
      currentParaId = para.id;
    }

    function updateChapterHighlight(chapterId) {
      for (const link of tocLinks) {
        const active = link.dataset.chapterId === chapterId;
        link.classList.toggle('is-current', active);
        if (active) {
          link.setAttribute('aria-current', 'location');
        } else {
          link.removeAttribute('aria-current');
        }
      }
    }

    function saveReadingPosition(para) {
      const viewportAnchor = window.innerHeight * VIEWPORT_ANCHOR_RATIO;
      const payload = {
        paraId: para.id,
        paraIndex: Number(para.dataset.paraIndex || '0'),
        withinParaOffset: Math.max(0, Math.round(getScrollTop() + viewportAnchor - para.offsetTop)),
      };
      saveJson(POSITION_KEY, payload);
    }

    function updateReadingState() {
      const current = getCurrentParagraph();
      if (!current) return;

      const total = paraBlocks.length || 1;
      const index = Number(current.dataset.paraIndex || '1');
      const pct = total <= 1 ? 100 : ((index - 1) / (total - 1)) * 100;
      const chapter = current.closest('.chapter');
      const chapterId = chapter?.id || '';
      const chapterTitle = chapter?.querySelector('.chapter-title')?.textContent?.trim() || 'Current position';
      const locText = `${chapterTitle} · ${index}/${total}`;
      const progressText = `${index}/${total} (${pct.toFixed(2)}%)`;

      bar.style.width = pct + '%';
      updateProgressBarVisual(pct);
      pctEl.textContent = progressText;
      tocLocInlineEl.textContent = locText;
      updateCurrentHighlight(current);
      updateChapterHighlight(chapterId);
      if (canSaveReadingPosition) {
        saveReadingPosition(current);
      }
    }

    function scheduleReadingState() {
      if (rafPending) return;
      rafPending = true;
      requestAnimationFrame(() => {
        rafPending = false;
        updateReadingState();
      });
    }

    function restoreOpenDetails() {
      const openKeys = new Set(safeParse(localStorage.getItem(DETAILS_KEY), []));
      for (const section of detailSections) {
        section.open = openKeys.has(section.dataset.detailKey);
      }
    }

    function persistOpenDetails() {
      const openKeys = detailSections
        .filter(section => section.open)
        .map(section => section.dataset.detailKey);
      saveJson(DETAILS_KEY, openKeys);
    }

    function restoreReadingPosition() {
      if (window.location.hash) {
        canSaveReadingPosition = true;
        scheduleReadingState();
        return;
      }

      const saved = safeParse(localStorage.getItem(POSITION_KEY), null);
      if (!saved) {
        canSaveReadingPosition = true;
        scheduleReadingState();
        return;
      }

      const target =
        document.getElementById(saved.paraId) ||
        paraBlocks[Math.max(0, Number(saved.paraIndex || 1) - 1)];

      if (!target) {
        canSaveReadingPosition = true;
        scheduleReadingState();
        return;
      }

      const viewportAnchor = window.innerHeight * VIEWPORT_ANCHOR_RATIO;
      const top = Math.max(
        0,
        target.offsetTop + Number(saved.withinParaOffset || 0) - viewportAnchor
      );
      window.scrollTo({ top, behavior: 'auto' });
      canSaveReadingPosition = true;
      scheduleReadingState();
    }

    paraBlocks.forEach((para, index) => {
      para.dataset.paraIndex = String(index + 1);
    });

    restoreOpenDetails();

    detailSections.forEach(section => {
      section.addEventListener('toggle', () => {
        persistOpenDetails();
        scheduleReadingState();
      });
    });

    const savedTheme = (() => {
      try {
        return localStorage.getItem(THEME_KEY);
      } catch (_) {
        return null;
      }
    })();
    applyTheme(savedTheme, false);

    themeToggle.addEventListener('click', () => {
      applyTheme(document.body.dataset.theme === 'legacy' ? 'current' : 'legacy');
    });
    tocToggle.addEventListener('click', () => {
      setTocOpen(!document.body.classList.contains('toc-open'));
    });
    tocBackdrop.addEventListener('click', () => setTocOpen(false));
    tocLinks.forEach(link => {
      link.addEventListener('click', () => setTocOpen(false));
    });

    window.addEventListener('keydown', event => {
      if (event.key === 'Escape') setTocOpen(false);
    });
    window.addEventListener('scroll', scheduleReadingState, { passive: true });
    window.addEventListener('resize', scheduleReadingState);
    window.addEventListener('hashchange', scheduleReadingState);
    window.addEventListener('load', () => {
      requestAnimationFrame(() => {
        requestAnimationFrame(() => {
          restoreReadingPosition();
        });
      });
    });
  </script>
</body>
</html>
"#;
