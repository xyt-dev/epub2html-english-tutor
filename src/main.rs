mod epub_parser;
mod html_gen;
mod llm_client;
mod state;
mod types;

use anyhow::{Context, Result};
use indicatif::{ProgressBar, ProgressStyle};
use llm_client::LlmClient;
use std::path::{Path, PathBuf};

// ─── Entry point ─────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // API key from environment variable
    let api_key = std::env::var("ANTHROPIC_AUTH_TOKEN")
        .context("ANTHROPIC_AUTH_TOKEN env var not set")?;

    // Determine the novel directory — allow override via CLI arg
    let novels_dir: PathBuf = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("LightNovels"));

    let output_dir = PathBuf::from("output");
    std::fs::create_dir_all(&output_dir)?;

    // Collect all epub files
    let epubs = collect_epubs(&novels_dir)?;
    if epubs.is_empty() {
        eprintln!("No .epub files found under {}", novels_dir.display());
        return Ok(());
    }
    println!("Found {} epub file(s) under {}", epubs.len(), novels_dir.display());

    let client = LlmClient::new(api_key);

    for epub_path in &epubs {
        println!("\n─────────────────────────────────────────");
        println!("Processing: {}", epub_path.display());
        match process_epub(epub_path, &output_dir, &client).await {
            Ok(_) => println!("  ✓ Done"),
            Err(e) => eprintln!("  ✗ Error: {:#}", e),
        }
    }

    Ok(())
}

// ─── Per-epub pipeline ────────────────────────────────────────────────────────

async fn process_epub(
    epub_path: &Path,
    output_dir: &Path,
    client: &LlmClient,
) -> Result<()> {
    // 1. Parse epub → Book
    println!("  [1/3] Parsing epub…");
    let book = epub_parser::parse_epub(epub_path)?;

    let total_paras: usize = book.chapters.iter().map(|c| c.paragraphs.len()).sum();
    println!(
        "  Book: \"{}\" | {} chapters | {} paragraphs",
        book.title,
        book.chapters.len(),
        total_paras
    );

    // 2. Generate HTML skeleton (or load existing)
    let html_path = output_dir.join(format!("{}.html", book.slug));
    let state_path = state::state_path(output_dir, &book.slug);

    println!("  [2/3] Generating HTML skeleton…");
    let mut html_content = if html_path.exists() {
        std::fs::read_to_string(&html_path)?
    } else {
        let initial_html = html_gen::generate_html(&book);
        std::fs::write(&html_path, &initial_html)?;
        initial_html
    };
    println!("  HTML → {}", html_path.display());

    // 3. LLM translation (resumable)
    println!("  [3/3] Translating paragraphs with Claude…");
    let mut st = state::load_state(&state_path)?;

    let pending: Vec<(&str, &str)> = book
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .filter(|p| !st.is_done(&p.id))
        .map(|p| (p.id.as_str(), p.text.as_str()))
        .collect();

    let already_done = total_paras - pending.len();
    println!(
        "  Progress: {}/{} done, {} remaining",
        already_done,
        total_paras,
        pending.len()
    );

    if pending.is_empty() {
        println!("  All paragraphs already translated.");
        return Ok(());
    }

    let pb = ProgressBar::new(pending.len() as u64);
    pb.set_style(
        ProgressStyle::with_template(
            "  {bar:40.cyan/blue} {pos}/{len} [{elapsed_precise}] {msg}",
        )
        .unwrap(),
    );

    // Build an id→para lookup for patching HTML
    let para_map: std::collections::HashMap<&str, &types::Paragraph> = book
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .map(|p| (p.id.as_str(), p))
        .collect();

    for (para_id, para_text) in &pending {
        pb.set_message(format!("id={}", para_id));

        match client.translate_paragraph(para_text).await {
            Ok(resp) => {
                // Patch HTML in-memory
                if let Some(para) = para_map.get(para_id) {
                    html_content = html_gen::patch_html(&html_content, para, &resp);
                }

                // Write HTML first (atomic: write tmp → rename)
                // Order matters: HTML before state. If we crash here, state won't record
                // this para as done, so next run re-translates it (harmless extra API call).
                // The reverse order would leave a permanent placeholder hole.
                let tmp = html_path.with_extension("html.tmp");
                std::fs::write(&tmp, &html_content)?;
                std::fs::rename(&tmp, &html_path)?;

                // Only after HTML is safely on disk, record in state
                st.mark_done(para_id.to_string(), resp);
                state::save_state(&state_path, &st)?;
            }
            Err(e) => {
                pb.println(format!("  [WARN] skipping {}: {:#}", para_id, e));
            }
        }
        pb.inc(1);
    }
    pb.finish_with_message("done");

    println!("  State → {}", state_path.display());
    println!("  HTML  → {}", html_path.display());
    Ok(())
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

fn collect_epubs(dir: &Path) -> Result<Vec<PathBuf>> {
    if !dir.exists() {
        anyhow::bail!(
            "Novel directory '{}' does not exist. Pass the path as the first argument.",
            dir.display()
        );
    }
    let mut epubs = Vec::new();
    visit_dir(dir, &mut epubs)?;
    epubs.sort();
    Ok(epubs)
}

fn visit_dir(dir: &Path, epubs: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, epubs)?;
        } else if path.extension().map(|e| e == "epub").unwrap_or(false) {
            epubs.push(path);
        }
    }
    Ok(())
}
