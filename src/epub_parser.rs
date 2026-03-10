use anyhow::{Context, Result};
use epub::doc::EpubDoc;
use scraper::{Html, Selector};
use slug::slugify;

use crate::types::{Book, Chapter, Paragraph};

/// Parse all epub files under `novels_dir` and return a list of Books.
pub fn parse_epub(epub_path: &std::path::Path) -> Result<Book> {
    let mut doc = EpubDoc::new(epub_path)
        .with_context(|| format!("Failed to open epub: {}", epub_path.display()))?;

    let title = doc
        .mdata("title")
        .unwrap_or_else(|| epub_path.file_stem().unwrap().to_string_lossy().to_string());

    let slug = slugify(&title);

    // Collect all spine items (chapters)
    let spine_len = doc.get_num_pages();
    let mut chapters = Vec::new();
    let mut chapter_index = 0usize;

    for page_idx in 0..spine_len {
        let _ = doc.set_current_page(page_idx);

        let content = match doc.get_current_str() {
            Ok(s) => s,
            Err(_) => continue,
        };

        let paras = extract_paragraphs(&content, &slug, chapter_index);
        if paras.is_empty() {
            continue;
        }

        let title_opt = extract_chapter_title(&content);

        chapters.push(Chapter {
            index: chapter_index,
            title: title_opt,
            paragraphs: paras,
        });
        chapter_index += 1;
    }

    Ok(Book {
        slug,
        title,
        chapters,
    })
}

/// Extract paragraphs from a chapter's XHTML/HTML content.
fn extract_paragraphs(html: &str, book_slug: &str, chapter_idx: usize) -> Vec<Paragraph> {
    let document = Html::parse_document(html);
    let p_sel = Selector::parse("p").unwrap();

    let mut paragraphs = Vec::new();
    let mut para_idx = 0usize;

    for element in document.select(&p_sel) {
        let text: String = element.text().collect::<Vec<_>>().join(" ");
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");

        // Skip very short lines (page numbers, section markers, etc.)
        if text.len() < 20 {
            continue;
        }
        // Skip lines that look like chapter headers repeated (all-caps short)
        let word_count = text.split_whitespace().count();
        if word_count < 4 && text.chars().all(|c| c.is_uppercase() || !c.is_alphabetic()) {
            continue;
        }

        let id = format!("{}-ch{:03}-p{:04}", book_slug, chapter_idx, para_idx);

        paragraphs.push(Paragraph { id, text });
        para_idx += 1;
    }

    paragraphs
}

/// Try to extract a chapter title from the HTML (h1/h2/h3).
fn extract_chapter_title(html: &str) -> Option<String> {
    let document = Html::parse_document(html);
    for tag in &["h1", "h2", "h3"] {
        if let Ok(sel) = Selector::parse(tag) {
            if let Some(el) = document.select(&sel).next() {
                let text: String = el.text().collect::<Vec<_>>().join(" ");
                let text = text.trim().to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }
    }
    None
}
