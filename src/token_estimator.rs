//! Offline token estimates for preflight counting and batch summaries.
//!
//! Claude's exact tokenizer is model-side. These helpers intentionally keep a
//! conservative local estimate so `--count` can stay API-free.

const ASCII_RUN_CHARS_PER_TOKEN: usize = 4;
const NON_ASCII_RUN_BYTES_PER_TOKEN: usize = 3;
const MESSAGE_ENVELOPE_TOKENS: usize = 12;

pub fn effective_text_chars(text: &str) -> usize {
    text.chars().filter(|c| !c.is_whitespace()).count()
}

pub fn estimate_message_input_tokens(system: &str, user_content: &str) -> usize {
    estimate_text_tokens(system) + estimate_text_tokens(user_content) + MESSAGE_ENVELOPE_TOKENS
}

pub fn estimate_text_tokens(text: &str) -> usize {
    let mut tokens = 0usize;
    let mut ascii_run_chars = 0usize;
    let mut non_ascii_run_bytes = 0usize;

    for ch in text.chars() {
        if ch.is_whitespace() {
            flush_runs(&mut tokens, &mut ascii_run_chars, &mut non_ascii_run_bytes);
            continue;
        }

        if is_han_char(ch) {
            flush_runs(&mut tokens, &mut ascii_run_chars, &mut non_ascii_run_bytes);
            tokens += 1;
            continue;
        }

        if ch.is_ascii_alphanumeric() {
            ascii_run_chars += 1;
            continue;
        }

        if is_word_connector(ch) && (ascii_run_chars > 0 || non_ascii_run_bytes > 0) {
            if ascii_run_chars > 0 {
                ascii_run_chars += 1;
            } else {
                non_ascii_run_bytes += ch.len_utf8();
            }
            continue;
        }

        if ch.is_alphabetic() || ch.is_numeric() {
            non_ascii_run_bytes += ch.len_utf8();
            continue;
        }

        flush_runs(&mut tokens, &mut ascii_run_chars, &mut non_ascii_run_bytes);
        tokens += 1;
    }

    flush_runs(&mut tokens, &mut ascii_run_chars, &mut non_ascii_run_bytes);
    tokens
}

pub fn is_han_char(ch: char) -> bool {
    matches!(
        ch,
        '\u{3400}'..='\u{4DBF}'
            | '\u{4E00}'..='\u{9FFF}'
            | '\u{F900}'..='\u{FAFF}'
            | '\u{20000}'..='\u{2A6DF}'
            | '\u{2A700}'..='\u{2B73F}'
            | '\u{2B740}'..='\u{2B81F}'
            | '\u{2B820}'..='\u{2CEAF}'
            | '\u{2CEB0}'..='\u{2EBEF}'
            | '\u{30000}'..='\u{3134F}'
    )
}

fn flush_runs(tokens: &mut usize, ascii_run_chars: &mut usize, non_ascii_run_bytes: &mut usize) {
    if *ascii_run_chars > 0 {
        *tokens += ceil_div(*ascii_run_chars, ASCII_RUN_CHARS_PER_TOKEN);
        *ascii_run_chars = 0;
    }
    if *non_ascii_run_bytes > 0 {
        *tokens += ceil_div(*non_ascii_run_bytes, NON_ASCII_RUN_BYTES_PER_TOKEN);
        *non_ascii_run_bytes = 0;
    }
}

fn ceil_div(value: usize, divisor: usize) -> usize {
    (value + divisor - 1) / divisor
}

fn is_word_connector(ch: char) -> bool {
    matches!(ch, '\'' | '-' | '’')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effective_chars_ignore_whitespace() {
        assert_eq!(effective_text_chars("ab c\n d\t"), 4);
    }

    #[test]
    fn estimates_han_chars_as_individual_tokens() {
        assert_eq!(estimate_text_tokens("你好學習"), 4);
    }

    #[test]
    fn estimates_ascii_words_by_length() {
        assert_eq!(estimate_text_tokens("Don't split well-known words."), 10);
    }
}
