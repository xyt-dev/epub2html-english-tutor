use crate::types::{Book, LlmResponse, Paragraph};
use html_escape::encode_text;
use std::fmt::Write as FmtWrite;

/// Generate a full HTML page for the given book.
/// Each paragraph gets placeholder `<details>` sections.
pub fn generate_html(book: &Book) -> String {
    let mut body = String::new();

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
            body.push_str(&render_para_block(para, None));
        }

        body.push_str("</section>\n");
    }

    HTML_TEMPLATE
        .replace("{{TITLE}}", &encode_text(&book.title))
        .replace("{{BODY}}", &body)
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
  <details class="ai-section translation-section">
    <summary><span class="section-icon">🈳</span> 译文</summary>
    <div class="ai-content">{translation_html}</div>
  </details>
  <details class="ai-section vocab-section">
    <summary><span class="section-icon">📚</span> 词汇 (IELTS 6.5+)</summary>
    <div class="ai-content">{vocab_html}</div>
  </details>
  <details class="ai-section chunk-section">
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

fn render_vocab(entries: &[crate::types::VocabEntry]) -> String {
    if entries.is_empty() {
        return "<p class=\"empty\">—</p>".to_string();
    }
    let mut s = String::from(r#"<table class="vocab-table"><thead><tr><th>单词</th><th>音标</th><th>词性</th><th>释义</th><th>例句</th></tr></thead><tbody>"#);
    for e in entries {
        s.push_str(&format!(
            "<tr><td class=\"word\">{}</td><td class=\"ipa\">{}</td><td class=\"pos\">{}</td><td>{}</td><td class=\"example\"><em>{}</em></td></tr>",
            encode_text(&e.word),
            encode_text(&e.ipa),
            encode_text(&e.pos),
            encode_text(&e.cn),
            encode_text(&e.example),
        ));
    }
    s.push_str("</tbody></table>");
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
      --accent:    #7aa2f7;
      --green:     #9ece6a;
      --yellow:    #e0af68;
      --red:       #f7768e;
      --cyan:      #7dcfff;
      --purple:    #bb9af7;
      --orange:    #ff9e64;
      --radius:    8px;
      font-size:   17px;
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
    a { color: var(--accent); }

    /* ── Progress bar (top) ────────────────────────────────────── */
    #progress-bar-wrap {
      position: fixed; top: 0; left: 0; width: 100%; height: 7px;
      background: var(--bg2); z-index: 100;
    }
    #progress-bar { height: 100%; background: var(--accent); width: 0%; transition: width .2s; }

    /* ── Chapter ───────────────────────────────────────────────── */
    .chapter { margin-bottom: 4rem; }
    .chapter-title {
      font-size: 1.6rem; color: var(--purple);
      border-bottom: 2px solid var(--border);
      padding-bottom: .4rem; margin-bottom: 2rem;
    }

    /* ── Paragraph block ───────────────────────────────────────── */
    .para-block {
      margin-bottom: 2rem;
      border-left: 3px solid var(--border);
      padding-left: 1rem;
      transition: border-color .2s;
    }
    .para-block[data-status="done"] { border-left-color: var(--green); }
    .para-block[data-status="pending"] { border-left-color: var(--border); }

    .original-text {
      font-size: 1rem;
      color: var(--text);
      margin-bottom: .6rem;
      text-align: justify;
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

    .translation-section > summary { background: #1e2940; color: var(--cyan); }
    .vocab-section      > summary { background: #201e30; color: var(--purple); }
    .chunk-section      > summary { background: #1e2a20; color: var(--green); }

    .ai-content {
      padding: .7rem 1rem;
      font-size: .9rem;
      font-family: 'Segoe UI', system-ui, sans-serif;
      line-height: 1.7;
      background: var(--surface);
    }

    /* Translation */
    .translation-section .ai-content p { color: var(--cyan); }

    /* Vocab table */
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
    .vocab-table .word    { color: var(--yellow); font-weight: 700; }
    .vocab-table .ipa     { color: var(--text-dim); font-family: monospace; }
    .vocab-table .pos     { color: var(--orange); font-style: italic; }
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
    @media (max-width: 600px) {
      body { font-size: 15px; padding: 1rem .8rem 4rem; }
      .vocab-table { font-size: .75rem; }
    }
  </style>
</head>
<body>
  <div id="progress-bar-wrap"><div id="progress-bar"></div></div>

  <h1 style="color:var(--accent);margin-bottom:2.5rem;font-size:2rem;">{{TITLE}}</h1>

  {{BODY}}

  <script>
    // Reading-progress bar
    window.addEventListener('scroll', () => {
      const h = document.documentElement;
      const pct = (h.scrollTop / (h.scrollHeight - h.clientHeight)) * 100;
      document.getElementById('progress-bar').style.width = pct + '%';
    });
  </script>
</body>
</html>
"#;
