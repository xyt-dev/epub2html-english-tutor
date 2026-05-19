mod epub_parser;
mod fs_utils;
mod html_gen;
mod llm_client;
mod markdown_parser;
mod parse_utils;
mod parser;
mod state;
mod text_parser;
mod token_estimator;
mod types;
mod ui;

use anyhow::{Context, Result};
use clap::{ArgAction, Parser};
use console::{measure_text_width, pad_str, Alignment, Term};
use indicatif::ProgressBar;
use llm_client::{LlmClient, TranslationRequest};
use parse_utils::ParseOptions;
use regex::Regex;
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;
use tokio::task::JoinSet;

#[derive(Parser, Debug)]
#[command(
    name = "epub-reader",
    version,
    about = "Convert EPUB/Markdown/Text books into annotated HTML with AI translation.",
    long_about = None,
    after_help = "Examples:\n  epub-reader ../Books\n  epub-reader novel.epub\n  epub-reader notes.md ./out\n  epub-reader --count ../Books\n  epub-reader --jobs 3 novel.epub\n  epub-reader --txt-hard-linebreaks notes.txt ./out\n  epub-reader --rebuild ../Books ./out"
)]
struct Args {
    #[arg(
        value_name = "INPUT",
        help = "Input file or directory (.epub/.md/.markdown/.txt)"
    )]
    input: PathBuf,

    #[arg(
        value_name = "OUTPUT",
        default_value = "output",
        help = "Output directory for HTML and state files"
    )]
    output_dir: PathBuf,

    #[arg(
        long,
        help = "Rebuild HTML from existing state files without API calls"
    )]
    rebuild: bool,

    #[arg(
        long,
        help = "Count translatable source text and exit without API calls or output files"
    )]
    count: bool,

    #[arg(
        short,
        long,
        help = "Print full LLM request and response bodies to stderr"
    )]
    verbose: bool,

    #[arg(
        long,
        default_value_t = 2,
        help = "Maximum number of concurrent translation requests"
    )]
    jobs: usize,

    #[arg(
        long,
        default_value_t = 0,
        help = "Delay in milliseconds before launching each translation request"
    )]
    request_delay_ms: u64,

    #[arg(
        long,
        default_value_t = DEFAULT_CONTEXT_PARAGRAPHS,
        help = "Number of preceding source paragraphs to include as context for each translation request"
    )]
    context_paragraphs: usize,

    #[arg(
        long,
        default_value_t = 2,
        help = "Minimum characters required for a text block without sentence punctuation"
    )]
    min_paragraph_chars: usize,

    #[arg(
        long,
        default_value_t = 12,
        help = "Maximum words to treat a short line as a book title candidate"
    )]
    title_max_words: usize,

    #[arg(
        long,
        default_value_t = 8,
        help = "Maximum words to treat an uppercase short line as a heading"
    )]
    heading_max_words: usize,

    #[arg(
        long,
        help = "In .txt files, treat each non-empty line as its own paragraph"
    )]
    txt_hard_linebreaks: bool,

    #[arg(
        long = "txt-no-sentence-split",
        action = ArgAction::SetFalse,
        default_value_t = true,
        help = "In .txt files, do not start a new paragraph after sentence-ending punctuation"
    )]
    txt_split_on_sentence_end: bool,
}

#[derive(Debug, Clone)]
struct JobOutcome {
    book_title: String,
    total_paragraphs: usize,
    completed: usize,
    html_path: PathBuf,
    state_path: Option<PathBuf>,
}

#[derive(Debug, Clone)]
struct TranslationOptions {
    jobs: usize,
    request_delay: Duration,
    context_paragraphs: usize,
}

#[derive(Debug, Clone)]
struct PendingParagraph {
    para_id: String,
    para_text: String,
}

#[derive(Debug, Clone)]
struct PendingBatch {
    book_title: String,
    context: Vec<PendingParagraph>,
    paragraphs: Vec<PendingParagraph>,
    metrics: BatchMetrics,
}

#[derive(Debug, Clone, Copy, Default)]
struct BatchMetrics {
    effective_chars: usize,
    input_tokens_estimate: usize,
}

#[derive(Debug, Clone, Copy, Default)]
struct BatchSummary {
    request_count: usize,
    effective_chars: usize,
    input_tokens_estimate: usize,
}

#[derive(Debug, Default, Clone)]
struct CountStats {
    books: usize,
    chapters: usize,
    paragraphs: usize,
    effective_chars: usize,
    chinese_chars: usize,
    english_words: usize,
    request_count: usize,
    input_tokens_estimate: usize,
}

#[derive(Debug, Clone)]
struct CountRow {
    title: String,
    stats: CountStats,
}

impl CountStats {
    fn add(&mut self, other: &Self) {
        self.books += other.books;
        self.chapters += other.chapters;
        self.paragraphs += other.paragraphs;
        self.effective_chars += other.effective_chars;
        self.chinese_chars += other.chinese_chars;
        self.english_words += other.english_words;
        self.request_count += other.request_count;
        self.input_tokens_estimate += other.input_tokens_estimate;
    }
}

#[derive(Debug)]
struct ParagraphTaskResult {
    para_id: String,
    outcome: std::result::Result<types::LlmResponse, String>,
}

#[derive(Debug)]
enum TranslationTaskResult {
    Completed {
        items: Vec<ParagraphTaskResult>,
        metrics: BatchMetrics,
    },
    RetryIndividually {
        book_title: String,
        context: Vec<PendingParagraph>,
        paragraphs: Vec<PendingParagraph>,
        metrics: BatchMetrics,
        error: String,
    },
}

const BATCH_TARGET_CHARS: usize = 5_000;
const BATCH_HARD_MAX_CHARS: usize = 7_000;
const BATCH_MAX_ITEMS: usize = 8;
const SINGLE_PARAGRAPH_CHARS: usize = 2_800;
const DEFAULT_CONTEXT_PARAGRAPHS: usize = 10;
const MAX_CONTEXT_PARAGRAPHS: usize = 20;

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    validate_args(&args)?;

    let parse_options = parse_options_from_args(&args);
    if args.count {
        return count_inputs(&args.input, &parse_options, args.context_paragraphs);
    }

    std::fs::create_dir_all(&args.output_dir)?;

    let translation_options = TranslationOptions {
        jobs: args.jobs,
        request_delay: Duration::from_millis(args.request_delay_ms),
        context_paragraphs: args.context_paragraphs,
    };

    ui::print_banner(&args.output_dir, args.rebuild);
    ui::print_kv("parse-rules", parse_options.summary());
    if !args.rebuild {
        ui::print_kv(
            "llm",
            format!(
                "{} job(s) · {}ms launch delay · context {} para(s) · adaptive batches target {} / max {} chars · max {} item(s)",
                translation_options.jobs,
                args.request_delay_ms,
                translation_options.context_paragraphs,
                BATCH_TARGET_CHARS,
                BATCH_HARD_MAX_CHARS,
                BATCH_MAX_ITEMS
            ),
        );
    }

    let inputs = collect_inputs(&args.input)?;
    if inputs.is_empty() {
        ui::print_error(format!(
            "No supported input files ({}) found under {}",
            parser::supported_extensions_summary(),
            args.input.display()
        ));
        return Ok(());
    }
    ui::print_input_summary(&args.input, inputs.len());

    let client = if args.rebuild {
        None
    } else {
        Some(LlmClient::new(
            std::env::var("ANTHROPIC_AUTH_TOKEN")
                .context("ANTHROPIC_AUTH_TOKEN env var not set")?,
            args.verbose,
        ))
    };

    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for (idx, input_path) in inputs.iter().enumerate() {
        ui::print_job_header(idx + 1, inputs.len(), input_path);

        let result = if args.rebuild {
            rebuild_html(input_path, &args.output_dir, &parse_options)
        } else {
            process_input(
                input_path,
                &args.output_dir,
                client.as_ref().unwrap(),
                &parse_options,
                &translation_options,
            )
            .await
        };

        match result {
            Ok(outcome) => {
                succeeded += 1;
                ui::print_success(format!(
                    "{} · {}/{} paragraphs ready",
                    outcome.book_title, outcome.completed, outcome.total_paragraphs
                ));
                ui::print_kv("html", outcome.html_path.display().to_string());
                if let Some(state_path) = outcome.state_path {
                    ui::print_kv("state", state_path.display().to_string());
                }
            }
            Err(err) => {
                failed += 1;
                ui::print_error(format!("{}: {:#}", input_path.display(), err));
            }
        }
    }

    ui::print_run_summary(succeeded, failed);
    Ok(())
}

fn rebuild_html(
    input_path: &Path,
    output_dir: &Path,
    parse_options: &ParseOptions,
) -> Result<JobOutcome> {
    ui::print_step("parse", "reading source content");
    let book = parser::parse_book(input_path, parse_options)?;
    let total_paragraphs: usize = book
        .chapters
        .iter()
        .map(|c| c.paragraphs.iter().filter(|p| p.is_translatable()).count())
        .sum();
    ui::print_book_summary(&book.title, book.chapters.len(), total_paragraphs);

    let state_path = state::state_path(output_dir, &book.slug);
    let html_path = output_dir.join(format!("{}.html", book.slug));

    ui::print_step("state", "loading saved responses");
    let st = state::load_state(&state_path)?;
    ui::print_kv(
        "loaded",
        format!("{} cached paragraph(s)", st.completed.len()),
    );

    ui::print_step("html", "rebuilding from skeleton");
    let mut html = html_gen::generate_html(&book);
    let para_map = build_para_map(&book);

    let pb = ProgressBar::new(st.completed.len() as u64);
    pb.set_style(ui::progress_style(false));
    pb.enable_steady_tick(Duration::from_millis(80));

    for (para_id, resp) in &st.completed {
        if let Some(para) = para_map.get(para_id.as_str()) {
            html = html_gen::patch_html(&html, para, resp);
        }
        pb.inc(1);
    }
    pb.finish_and_clear();

    fs_utils::atomic_write(&html_path, html.as_bytes())?;

    Ok(JobOutcome {
        book_title: book.title,
        total_paragraphs,
        completed: st.completed.len(),
        html_path,
        state_path: state_path.exists().then_some(state_path),
    })
}

async fn process_input(
    input_path: &Path,
    output_dir: &Path,
    client: &LlmClient,
    parse_options: &ParseOptions,
    translation_options: &TranslationOptions,
) -> Result<JobOutcome> {
    ui::print_step("parse", "reading source content");
    let book = parser::parse_book(input_path, parse_options)?;
    let total_paragraphs: usize = book
        .chapters
        .iter()
        .map(|c| c.paragraphs.iter().filter(|p| p.is_translatable()).count())
        .sum();
    ui::print_book_summary(&book.title, book.chapters.len(), total_paragraphs);

    let html_path = output_dir.join(format!("{}.html", book.slug));
    let state_path = state::state_path(output_dir, &book.slug);

    ui::print_step("html", "loading or creating skeleton");
    let mut html_content = if html_path.exists() {
        std::fs::read_to_string(&html_path)?
    } else {
        let initial_html = html_gen::generate_html(&book);
        fs_utils::atomic_write(&html_path, initial_html.as_bytes())?;
        initial_html
    };

    ui::print_step("state", "loading resumable progress");
    let mut st = state::load_state(&state_path)?;
    ui::print_kv(
        "loaded",
        format!("{} cached paragraph(s)", st.completed.len()),
    );

    let all_translatable: Vec<PendingParagraph> = book
        .chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .filter(|p| p.is_translatable() && !p.text.trim().is_empty())
        .map(|p| PendingParagraph {
            para_id: p.id.clone(),
            para_text: p.text.clone(),
        })
        .collect();
    let pending: Vec<PendingParagraph> = all_translatable
        .iter()
        .filter(|p| !st.is_done(&p.para_id))
        .cloned()
        .collect();

    let already_done = total_paragraphs.saturating_sub(pending.len());
    ui::print_kv(
        "progress",
        format!("{} done · {} remaining", already_done, pending.len()),
    );

    if pending.is_empty() {
        return Ok(JobOutcome {
            book_title: book.title,
            total_paragraphs,
            completed: already_done,
            html_path,
            state_path: state_path.exists().then_some(state_path),
        });
    }

    ui::print_step("translate", "requesting Claude in adaptive batches");
    let pb = ProgressBar::new(pending.len() as u64);
    pb.set_style(ui::progress_style(true));
    pb.enable_steady_tick(Duration::from_millis(80));

    let para_map = build_para_map(&book);
    let pending_batches = build_translation_batches(
        book.title.as_str(),
        pending,
        &all_translatable,
        translation_options.context_paragraphs,
    );
    let pending_summary = summarize_batches(&pending_batches);
    ui::print_kv(
        "batching",
        format!(
            "{} request(s) · {} char(s) · ~{} input token(s) queued",
            pending_summary.request_count,
            pending_summary.effective_chars,
            pending_summary.input_tokens_estimate
        ),
    );
    let mut join_set = JoinSet::new();
    let mut pending_queue = VecDeque::from(pending_batches);
    let mut launched_any = false;

    fill_translation_queue(
        &mut join_set,
        &mut pending_queue,
        client,
        translation_options,
        &mut launched_any,
    )
    .await;
    if !join_set.is_empty() {
        pb.set_message(format!("active={} · waiting first batch", join_set.len()));
    }

    while let Some(joined) = join_set.join_next().await {
        let task = joined.context("translation worker panicked")?;
        let (items, batch_metrics) = match task {
            TranslationTaskResult::Completed { items, metrics } => (items, metrics),
            TranslationTaskResult::RetryIndividually {
                book_title,
                context,
                paragraphs,
                metrics,
                error,
            } => {
                let batch_size = paragraphs.len();
                push_individual_batches_front(
                    &mut pending_queue,
                    book_title,
                    context,
                    paragraphs,
                    translation_options.context_paragraphs,
                );
                fill_translation_queue(
                    &mut join_set,
                    &mut pending_queue,
                    client,
                    translation_options,
                    &mut launched_any,
                )
                .await;
                pb.set_message(format!(
                    "active={} · split batch={} · ~{} tokens",
                    join_set.len(),
                    batch_size,
                    metrics.input_tokens_estimate
                ));
                pb.println(ui::warn_text(format!(
                    "split batch of {} into single requests: {}",
                    batch_size, error
                )));
                continue;
            }
        };
        let batch_size = items.len();
        let last_id = items
            .last()
            .map(|item| abbreviate_para_id(&item.para_id))
            .unwrap_or_else(|| "-".to_string());

        for item in items {
            match item.outcome {
                Ok(resp) => {
                    if let Some(para) = para_map.get(item.para_id.as_str()) {
                        html_content = html_gen::patch_html(&html_content, para, &resp);
                    }

                    fs_utils::atomic_write(&html_path, html_content.as_bytes())?;
                    st.mark_done(item.para_id.clone(), resp);
                    state::save_state(&state_path, &st)?;
                }
                Err(err) => {
                    pb.println(ui::warn_text(format!("skipping {}: {}", item.para_id, err)));
                }
            }
            pb.inc(1);
        }

        fill_translation_queue(
            &mut join_set,
            &mut pending_queue,
            client,
            translation_options,
            &mut launched_any,
        )
        .await;

        if join_set.is_empty() {
            pb.set_message("finalizing".to_string());
        } else {
            pb.set_message(format!(
                "active={} · batch={} · ~{} tokens · last={}",
                join_set.len(),
                batch_size,
                batch_metrics.input_tokens_estimate,
                last_id
            ));
        }
    }
    pb.finish_and_clear();

    Ok(JobOutcome {
        book_title: book.title,
        total_paragraphs,
        completed: st.completed.len(),
        html_path,
        state_path: state_path.exists().then_some(state_path),
    })
}

fn build_para_map<'a>(book: &'a types::Book) -> HashMap<&'a str, &'a types::Paragraph> {
    book.chapters
        .iter()
        .flat_map(|c| c.paragraphs.iter())
        .filter(|p| p.is_translatable())
        .map(|p| (p.id.as_str(), p))
        .collect()
}

fn count_inputs(
    input_root: &Path,
    parse_options: &ParseOptions,
    context_paragraphs: usize,
) -> Result<()> {
    let inputs = collect_inputs(input_root)?;
    ui::print_input_summary(input_root, inputs.len());

    let mut total = CountStats::default();
    let mut rows = Vec::new();
    let mut succeeded = 0usize;
    let mut failed = 0usize;

    for input_path in &inputs {
        match parser::parse_book(input_path, parse_options) {
            Ok(book) => {
                let stats = count_book(&book, context_paragraphs);
                let title = book.title;
                succeeded += 1;
                total.add(&stats);
                rows.push(CountRow { title, stats });
            }
            Err(err) => {
                failed += 1;
                ui::print_error(format!("{}: {:#}", input_path.display(), err));
            }
        }
    }

    if !rows.is_empty() {
        println!();
        print_count_table(&rows, (rows.len() > 1).then_some(&total));
    }
    ui::print_run_summary(succeeded, failed);
    Ok(())
}

fn print_count_table(rows: &[CountRow], total: Option<&CountStats>) {
    ui::print_step("count", "estimated request size");
    for line in render_count_output(rows, total, terminal_width()) {
        println!("{}", line);
    }
}

fn render_count_output(
    rows: &[CountRow],
    total: Option<&CountStats>,
    terminal_width: usize,
) -> Vec<String> {
    let title_width = rows
        .iter()
        .map(|row| measure_text_width(&row.title))
        .chain(total.map(|_| measure_text_width("total")))
        .max()
        .unwrap_or_else(|| measure_text_width("book"))
        .max(measure_text_width("book"));
    let table = render_count_table(rows, total, title_width);
    let table_width = table
        .iter()
        .map(|line| measure_text_width(line))
        .max()
        .unwrap_or_default();

    if table_width <= terminal_width {
        table
    } else {
        render_count_list(rows, total)
    }
}

fn render_count_table(
    rows: &[CountRow],
    total: Option<&CountStats>,
    title_width: usize,
) -> Vec<String> {
    let headers = ["book", "ch", "para", "chars", "zh", "en", "req", "~tokens"];
    let aligns = [
        Alignment::Left,
        Alignment::Right,
        Alignment::Right,
        Alignment::Right,
        Alignment::Right,
        Alignment::Right,
        Alignment::Right,
        Alignment::Right,
    ];

    let body = rows
        .iter()
        .map(|row| count_table_cells(&row.title, &row.stats, title_width))
        .collect::<Vec<_>>();
    let total_row = total.map(|stats| count_table_cells("total", stats, title_width));

    let mut widths = headers
        .iter()
        .map(|header| measure_text_width(header))
        .collect::<Vec<_>>();
    for row in body.iter().chain(total_row.iter()) {
        for (idx, cell) in row.iter().enumerate() {
            widths[idx] = widths[idx].max(measure_text_width(cell));
        }
    }

    let rule = render_table_rule(&widths);
    let mut lines = Vec::with_capacity(body.len() + 5);
    lines.push(rule.clone());
    lines.push(render_table_row(&headers, &widths, &aligns));
    lines.push(rule.clone());
    for row in &body {
        lines.push(render_table_row(row, &widths, &aligns));
    }
    if let Some(row) = &total_row {
        lines.push(rule.clone());
        lines.push(render_table_row(row, &widths, &aligns));
    }
    lines.push(rule);
    lines
}

fn count_table_cells(title: &str, stats: &CountStats, title_width: usize) -> Vec<String> {
    vec![
        pad_str(title, title_width, Alignment::Left, None).to_string(),
        format_count(stats.chapters),
        format_count(stats.paragraphs),
        format_count(stats.effective_chars),
        format_count(stats.chinese_chars),
        format_count(stats.english_words),
        format_count(stats.request_count),
        format!("~{}", format_count(stats.input_tokens_estimate)),
    ]
}

fn render_count_list(rows: &[CountRow], total: Option<&CountStats>) -> Vec<String> {
    let mut lines = Vec::new();

    for row in rows {
        lines.extend(render_count_list_item(&row.title, &row.stats));
    }
    if let Some(stats) = total {
        if !lines.is_empty() {
            lines.push(String::new());
        }
        lines.extend(render_count_list_item("total", stats));
    }

    lines
}

fn render_count_list_item(title: &str, stats: &CountStats) -> Vec<String> {
    vec![
        title.to_string(),
        format!(
            "  para={} chars={} req={} ~tokens={}",
            format_count(stats.paragraphs),
            format_count(stats.effective_chars),
            format_count(stats.request_count),
            format_count(stats.input_tokens_estimate)
        ),
        format!(
            "  ch={} zh={} en={}",
            format_count(stats.chapters),
            format_count(stats.chinese_chars),
            format_count(stats.english_words)
        ),
    ]
}

fn terminal_width() -> usize {
    std::env::var("COLUMNS")
        .ok()
        .and_then(|cols| cols.parse::<usize>().ok())
        .filter(|cols| *cols > 0)
        .unwrap_or_else(|| Term::stdout().size().1 as usize)
}

fn render_table_rule(widths: &[usize]) -> String {
    let parts = widths
        .iter()
        .map(|width| "-".repeat(width + 2))
        .collect::<Vec<_>>();
    format!("+{}+", parts.join("+"))
}

fn render_table_row<S: AsRef<str>>(cells: &[S], widths: &[usize], aligns: &[Alignment]) -> String {
    let parts = cells
        .iter()
        .zip(widths.iter())
        .zip(aligns.iter())
        .map(|((cell, width), align)| {
            format!(
                " {} ",
                pad_str(cell.as_ref(), *width, *align, None).as_ref()
            )
        })
        .collect::<Vec<_>>();
    format!("|{}|", parts.join("|"))
}

fn format_count(value: usize) -> String {
    let raw = value.to_string();
    let mut out = String::with_capacity(raw.len() + raw.len() / 3);
    let first_group_len = raw.len() % 3;

    for (idx, ch) in raw.chars().enumerate() {
        if idx > 0
            && (idx == first_group_len
                || (idx > first_group_len && (idx - first_group_len) % 3 == 0))
        {
            out.push(',');
        }
        out.push(ch);
    }

    out
}

fn count_book(book: &types::Book, context_paragraphs: usize) -> CountStats {
    let mut stats = CountStats {
        books: 1,
        chapters: book.chapters.len(),
        ..Default::default()
    };

    let pending = book
        .chapters
        .iter()
        .flat_map(|chapter| chapter.paragraphs.iter())
        .filter(|para| para.is_translatable() && !para.text.trim().is_empty())
        .map(|para| PendingParagraph {
            para_id: para.id.clone(),
            para_text: para.text.clone(),
        })
        .collect::<Vec<_>>();

    for para in &pending {
        stats.paragraphs += 1;
        stats.effective_chars += token_estimator::effective_text_chars(&para.para_text);
        stats.chinese_chars += count_chinese_chars(&para.para_text);
        stats.english_words += count_english_words(&para.para_text);
    }

    let batch_summary = summarize_batches(&build_translation_batches(
        book.title.as_str(),
        pending.clone(),
        &pending,
        context_paragraphs,
    ));
    stats.request_count = batch_summary.request_count;
    stats.input_tokens_estimate = batch_summary.input_tokens_estimate;

    stats
}

async fn fill_translation_queue(
    join_set: &mut JoinSet<TranslationTaskResult>,
    pending_queue: &mut VecDeque<PendingBatch>,
    client: &LlmClient,
    options: &TranslationOptions,
    launched_any: &mut bool,
) {
    while join_set.len() < options.jobs {
        let Some(job) = pending_queue.pop_front() else {
            break;
        };

        if *launched_any && !options.request_delay.is_zero() {
            tokio::time::sleep(options.request_delay).await;
        }

        let client = client.clone();
        join_set.spawn(async move {
            let metrics = job.metrics;
            let book_title = job.book_title;
            let context_items = job
                .context
                .iter()
                .map(|paragraph| TranslationRequest {
                    id: paragraph.para_id.as_str(),
                    text: paragraph.para_text.as_str(),
                })
                .collect::<Vec<_>>();
            let request_items = job
                .paragraphs
                .iter()
                .map(|paragraph| TranslationRequest {
                    id: paragraph.para_id.as_str(),
                    text: paragraph.para_text.as_str(),
                })
                .collect::<Vec<_>>();

            match client
                .translate_batch(book_title.as_str(), &context_items, &request_items)
                .await
            {
                Ok(responses) => {
                    let items = responses
                        .into_iter()
                        .map(|response| ParagraphTaskResult {
                            para_id: response.id,
                            outcome: Ok(response.response),
                        })
                        .collect();
                    TranslationTaskResult::Completed { items, metrics }
                }
                Err(batch_err) if job.paragraphs.len() > 1 => {
                    TranslationTaskResult::RetryIndividually {
                        book_title,
                        paragraphs: job.paragraphs,
                        context: job.context,
                        metrics,
                        error: format!("{:#}", batch_err),
                    }
                }
                Err(batch_err) => {
                    let items = job
                        .paragraphs
                        .into_iter()
                        .map(|paragraph| ParagraphTaskResult {
                            para_id: paragraph.para_id,
                            outcome: Err(format!("{:#}", batch_err)),
                        })
                        .collect();
                    TranslationTaskResult::Completed { items, metrics }
                }
            }
        });
        *launched_any = true;
    }
}

fn push_individual_batches_front(
    pending_queue: &mut VecDeque<PendingBatch>,
    book_title: String,
    context: Vec<PendingParagraph>,
    paragraphs: Vec<PendingParagraph>,
    context_paragraphs: usize,
) {
    let mut recent = context;
    let mut batches = Vec::with_capacity(paragraphs.len());

    for paragraph in paragraphs {
        let batch_context = tail_context(&recent, context_paragraphs);
        batches.push(make_pending_batch(
            vec![paragraph.clone()],
            batch_context,
            book_title.as_str(),
        ));
        recent.push(paragraph);
    }

    for batch in batches.into_iter().rev() {
        pending_queue.push_front(batch);
    }
}

fn build_translation_batches(
    book_title: &str,
    pending: Vec<PendingParagraph>,
    all_translatable: &[PendingParagraph],
    context_paragraphs: usize,
) -> Vec<PendingBatch> {
    let mut batches = Vec::new();
    let mut iter = pending.into_iter().peekable();

    while let Some(first) = iter.next() {
        let batch_context =
            context_for_batch(first.para_id.as_str(), all_translatable, context_paragraphs);
        let mut total_chars = token_estimator::effective_text_chars(&first.para_text);
        let mut paragraphs = vec![first];

        if total_chars > SINGLE_PARAGRAPH_CHARS {
            batches.push(make_pending_batch(paragraphs, batch_context, book_title));
            continue;
        }

        while paragraphs.len() < BATCH_MAX_ITEMS {
            if paragraphs.len() >= 2 && total_chars >= BATCH_TARGET_CHARS {
                break;
            }

            let Some(next) = iter.peek() else {
                break;
            };

            let next_chars = token_estimator::effective_text_chars(&next.para_text);
            if next_chars > SINGLE_PARAGRAPH_CHARS {
                break;
            }

            if total_chars + next_chars > BATCH_HARD_MAX_CHARS {
                break;
            }

            total_chars += next_chars;
            paragraphs.push(iter.next().unwrap());
        }

        batches.push(make_pending_batch(paragraphs, batch_context, book_title));
    }

    batches
}

fn make_pending_batch(
    paragraphs: Vec<PendingParagraph>,
    context: Vec<PendingParagraph>,
    book_title: &str,
) -> PendingBatch {
    let metrics = estimate_batch_metrics(book_title, &context, &paragraphs);
    PendingBatch {
        book_title: book_title.to_string(),
        context,
        paragraphs,
        metrics,
    }
}

fn context_for_batch(
    first_para_id: &str,
    all_translatable: &[PendingParagraph],
    context_paragraphs: usize,
) -> Vec<PendingParagraph> {
    if context_paragraphs == 0 {
        return Vec::new();
    }

    let Some(index) = all_translatable
        .iter()
        .position(|paragraph| paragraph.para_id == first_para_id)
    else {
        return Vec::new();
    };
    let start = index.saturating_sub(context_paragraphs);
    all_translatable[start..index].to_vec()
}

fn tail_context(recent: &[PendingParagraph], context_paragraphs: usize) -> Vec<PendingParagraph> {
    if context_paragraphs == 0 {
        return Vec::new();
    }

    let start = recent.len().saturating_sub(context_paragraphs);
    recent[start..].to_vec()
}

fn count_chinese_chars(text: &str) -> usize {
    text.chars()
        .filter(|&ch| token_estimator::is_han_char(ch))
        .count()
}

fn estimate_batch_metrics(
    book_title: &str,
    context: &[PendingParagraph],
    paragraphs: &[PendingParagraph],
) -> BatchMetrics {
    let effective_chars = paragraphs
        .iter()
        .map(|paragraph| token_estimator::effective_text_chars(&paragraph.para_text))
        .sum();
    let context_items = context
        .iter()
        .map(|paragraph| TranslationRequest {
            id: paragraph.para_id.as_str(),
            text: paragraph.para_text.as_str(),
        })
        .collect::<Vec<_>>();
    let request_items = paragraphs
        .iter()
        .map(|paragraph| TranslationRequest {
            id: paragraph.para_id.as_str(),
            text: paragraph.para_text.as_str(),
        })
        .collect::<Vec<_>>();
    let input_tokens_estimate =
        llm_client::estimate_translation_input_tokens(book_title, &context_items, &request_items);

    BatchMetrics {
        effective_chars,
        input_tokens_estimate,
    }
}

fn summarize_batches(batches: &[PendingBatch]) -> BatchSummary {
    let mut summary = BatchSummary::default();

    for batch in batches {
        summary.request_count += 1;
        summary.effective_chars += batch.metrics.effective_chars;
        summary.input_tokens_estimate += batch.metrics.input_tokens_estimate;
    }

    summary
}

fn count_english_words(text: &str) -> usize {
    static ENGLISH_WORD_RE: OnceLock<Regex> = OnceLock::new();
    let re = ENGLISH_WORD_RE
        .get_or_init(|| Regex::new(r"[A-Za-z][A-Za-z0-9]*(?:['’-][A-Za-z0-9]+)*").unwrap());
    re.find_iter(text).count()
}

fn parse_options_from_args(args: &Args) -> ParseOptions {
    ParseOptions {
        min_paragraph_chars: args.min_paragraph_chars,
        title_max_words: args.title_max_words,
        short_heading_max_words: args.heading_max_words,
        txt_hard_linebreaks: args.txt_hard_linebreaks,
        txt_split_on_sentence_end: args.txt_split_on_sentence_end,
    }
}

fn validate_args(args: &Args) -> Result<()> {
    if args.count && args.rebuild {
        anyhow::bail!("--count cannot be used together with --rebuild");
    }
    if !args.count && args.jobs == 0 {
        anyhow::bail!("--jobs must be at least 1");
    }
    if !args.count && args.jobs > 16 {
        anyhow::bail!("--jobs must be 16 or smaller");
    }
    if args.context_paragraphs > MAX_CONTEXT_PARAGRAPHS {
        anyhow::bail!(
            "--context-paragraphs must be {} or smaller",
            MAX_CONTEXT_PARAGRAPHS
        );
    }
    if args.min_paragraph_chars == 0 {
        anyhow::bail!("--min-paragraph-chars must be at least 1");
    }
    if args.title_max_words == 0 {
        anyhow::bail!("--title-max-words must be at least 1");
    }
    if args.heading_max_words == 0 {
        anyhow::bail!("--heading-max-words must be at least 1");
    }
    Ok(())
}

fn abbreviate_para_id(para_id: &str) -> String {
    const MAX_LEN: usize = 28;
    if para_id.len() <= MAX_LEN {
        return para_id.to_string();
    }
    format!("…{}", &para_id[para_id.len() - (MAX_LEN - 1)..])
}

fn collect_inputs(path: &Path) -> Result<Vec<PathBuf>> {
    if !path.exists() {
        anyhow::bail!("Path '{}' does not exist.", path.display());
    }

    if path.is_file() {
        parser::validate_requested_input(path)?;
        return Ok(vec![path.to_path_buf()]);
    }

    let mut inputs = Vec::new();
    visit_dir(path, &mut inputs)?;
    if inputs.is_empty() {
        anyhow::bail!(
            "No supported input files ({}) found in '{}'.",
            parser::supported_extensions_summary(),
            path.display()
        );
    }

    inputs.sort();
    Ok(inputs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(id: &str, len: usize) -> PendingParagraph {
        PendingParagraph {
            para_id: id.to_string(),
            para_text: "a".repeat(len),
        }
    }

    fn batches(pending: Vec<PendingParagraph>) -> Vec<PendingBatch> {
        build_translation_batches(
            "Sample Book",
            pending.clone(),
            &pending,
            DEFAULT_CONTEXT_PARAGRAPHS,
        )
    }

    #[test]
    fn batching_respects_max_items_cap() {
        let pending = (0..=BATCH_MAX_ITEMS)
            .map(|idx| pending(&format!("p{}", idx), 300))
            .collect::<Vec<_>>();

        let batches = batches(pending);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].paragraphs.len(), BATCH_MAX_ITEMS);
        assert_eq!(batches[1].paragraphs.len(), 1);
    }

    #[test]
    fn batching_includes_recent_source_context() {
        let pending = (0..=BATCH_MAX_ITEMS)
            .map(|idx| pending(&format!("p{}", idx), 300))
            .collect::<Vec<_>>();

        let batches = build_translation_batches("Sample Book", pending.clone(), &pending, 2);

        assert!(batches[0].context.is_empty());
        assert_eq!(
            batches[1]
                .context
                .iter()
                .map(|paragraph| paragraph.para_id.clone())
                .collect::<Vec<_>>(),
            vec![
                format!("p{}", BATCH_MAX_ITEMS - 2),
                format!("p{}", BATCH_MAX_ITEMS - 1)
            ]
        );
    }

    #[test]
    fn batching_stops_near_target_total_chars() {
        let pending = vec![
            pending("p1", 2_600),
            pending("p2", 2_400),
            pending("p3", 300),
        ];

        let batches = batches(pending);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].paragraphs.len(), 2);
        assert_eq!(batches[1].paragraphs.len(), 1);
    }

    #[test]
    fn batching_respects_hard_char_limit() {
        let pending = (0..3)
            .map(|idx| pending(&format!("p{}", idx), 2_400))
            .collect::<Vec<_>>();

        let batches = batches(pending);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].paragraphs.len(), 2);
        assert_eq!(batches[1].paragraphs.len(), 1);
    }

    #[test]
    fn oversized_paragraphs_are_sent_alone() {
        let pending = vec![pending("p1", 2_900), pending("p2", 2_950)];
        let batches = batches(pending);

        assert_eq!(batches.len(), 2);
        assert!(batches.iter().all(|batch| batch.paragraphs.len() == 1));
    }

    #[test]
    fn effective_text_chars_ignores_whitespace() {
        assert_eq!(token_estimator::effective_text_chars("ab c\n d\t"), 4);
    }

    #[test]
    fn counts_chinese_chars_without_punctuation() {
        assert_eq!(count_chinese_chars("你好，world！學習"), 4);
    }

    #[test]
    fn counts_english_words_with_contractions_and_hyphens() {
        assert_eq!(
            count_english_words("Don't split well-known words, but count API v2."),
            8
        );
    }

    #[test]
    fn count_book_skips_code_blocks() {
        let book = types::Book {
            slug: "sample".to_string(),
            title: "Sample".to_string(),
            chapters: vec![types::Chapter {
                index: 1,
                title: Some("One".to_string()),
                paragraphs: vec![
                    types::Paragraph {
                        id: "p1".to_string(),
                        text: "Hello world 你好".to_string(),
                        kind: types::ParagraphKind::Text,
                    },
                    types::Paragraph {
                        id: "p2".to_string(),
                        text: "fn main() {}".to_string(),
                        kind: types::ParagraphKind::CodeBlock {
                            language: Some("rust".to_string()),
                        },
                    },
                ],
            }],
        };

        let stats = count_book(&book, DEFAULT_CONTEXT_PARAGRAPHS);
        assert_eq!(stats.books, 1);
        assert_eq!(stats.chapters, 1);
        assert_eq!(stats.paragraphs, 1);
        assert_eq!(stats.chinese_chars, 2);
        assert_eq!(stats.english_words, 2);
        assert_eq!(stats.request_count, 1);
        assert!(stats.input_tokens_estimate > 0);
    }

    #[test]
    fn batching_records_token_estimates() {
        let pending = vec![pending("p1", 300), pending("p2", 200)];
        let batches = batches(pending);

        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].metrics.effective_chars, 500);
        assert!(batches[0].metrics.input_tokens_estimate > 0);
    }

    #[test]
    fn formats_count_values_with_grouping() {
        assert_eq!(format_count(0), "0");
        assert_eq!(format_count(999), "999");
        assert_eq!(format_count(1_000), "1,000");
        assert_eq!(format_count(123_456_789), "123,456,789");
    }

    #[test]
    fn count_table_includes_total_row() {
        let stats = CountStats {
            books: 1,
            chapters: 2,
            paragraphs: 3,
            effective_chars: 4_000,
            chinese_chars: 5,
            english_words: 6,
            request_count: 7,
            input_tokens_estimate: 8_900,
        };
        let rows = vec![CountRow {
            title: "A Very Long Book Title That Should Be Truncated In Count Tables".to_string(),
            stats: stats.clone(),
        }];

        let table = render_count_output(&rows, Some(&stats), 120).join("\n");
        assert!(table.contains("book"));
        assert!(table.contains("total"));
        assert!(table.contains("4,000"));
        assert!(table.contains("A Very Long Book Title That Should Be Truncated In Count Tables"));
    }

    #[test]
    fn count_output_uses_list_when_terminal_is_narrow() {
        let stats = CountStats {
            books: 1,
            chapters: 2,
            paragraphs: 3,
            effective_chars: 4_000,
            chinese_chars: 5,
            english_words: 6,
            request_count: 7,
            input_tokens_estimate: 8_900,
        };
        let rows = vec![CountRow {
            title: "Sample Book With Full Title".to_string(),
            stats,
        }];

        let output = render_count_output(&rows, None, 50).join("\n");
        assert!(!output.contains('|'));
        assert!(output.contains("Sample Book With Full Title"));
        assert!(output.contains("~tokens=8,900"));
    }
}

fn visit_dir(dir: &Path, inputs: &mut Vec<PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            visit_dir(&path, inputs)?;
        } else if parser::is_enabled_input(&path) {
            inputs.push(path);
        }
    }
    Ok(())
}
