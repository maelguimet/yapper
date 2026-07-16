//! Sanitize and normalize free-form text before TTS (social/X paste, acronyms).
//!
//! Rules (keep aligned with `python/yapper_tts/sanitize.py` + `fixtures/sanitize/`):
//! - Replace `http(s)://…` and `www.…` with the short speakable placeholder `link`.
//! - Drop standalone `#hashtag` tokens and `@handle` lines/tokens (prose kept).
//! - Cap unbroken tokens at [`MAX_UNBROKEN_TOKEN_CHARS`] by inserting spaces
//!   (segmenter also hard-splits anything still over the TTS chunk limit).
//! - Expand common initialisms such as AI/RLHF/TTS into spoken letters.
//! - Turn parenthetical delimiters into comma pauses while keeping their text.
//! - Keep FR letters/accents; strip emoji and other unsupported glyphs.

mod parentheticals;

use parentheticals::normalize_parenthetical_pauses;

/// Exact B21 fixture (social/X paste that must not crash the pipeline).
pub const B21_SOCIAL_FIXTURE: &str = "\
ILIAS ISM
@illyism
This doesn't work in EU btw 🫡

Neither the GPT-Live from today

The permanent EUnderclass is already here 🇪🇺";

/// B25 pronunciation fixture.
pub const B25_TTS_FIXTURE: &str = "This is a test. Does the TTS work?";

/// Max length (Unicode scalars) of one unbroken token after sanitize.
/// Longer runs (base64, minified code, leftover URLs) are space-split here.
pub const MAX_UNBROKEN_TOKEN_CHARS: usize = 80;

/// Short speakable stand-in for stripped URLs.
const URL_PLACEHOLDER: &str = "link";

/// Sanitize messy social/X text for Chatterbox: strip handles/URLs/hashtags,
/// drop unsupported glyphs, collapse blank lines, cap giant tokens, expand a
/// few acronyms so they are not misread.
///
/// Never panics; empty input → empty output.
pub fn sanitize_for_tts(text: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    for raw_line in text.lines() {
        let line = sanitize_line(raw_line);
        if !line.is_empty() {
            parts.push(line);
        }
    }
    collapse_ws(&parts.join(" "))
}

fn sanitize_line(line: &str) -> String {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    if is_handle_only(trimmed) {
        return String::new();
    }
    let mut s = strip_inline_handles(trimmed);
    s = replace_urls(&s);
    s = s.chars().filter(|c| keep_char(*c)).collect();
    let s = collapse_ws(&s);
    let s = drop_standalone_hashtags(&s);
    let s = normalize_parenthetical_pauses(&s);
    let s = cap_unbroken_tokens(&s, MAX_UNBROKEN_TOKEN_CHARS);
    expand_acronyms(&s)
}

/// Drop `@name` tokens; a bare `@` becomes spoken "at".
fn strip_inline_handles(trimmed: &str) -> String {
    let mut s = String::with_capacity(trimmed.len());
    let mut chars = trimmed.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '@' {
            let mut saw = false;
            while let Some(&n) = chars.peek() {
                if n.is_ascii_alphanumeric() || n == '_' {
                    saw = true;
                    chars.next();
                } else {
                    break;
                }
            }
            if !saw {
                s.push_str("at");
            }
            continue;
        }
        s.push(c);
    }
    s
}

/// Replace URL-like runs with [`URL_PLACEHOLDER`] so they are not spoken as garbage.
fn replace_urls(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0usize;
    while i < chars.len() {
        if let Some(end) = match_url_at(&chars, i) {
            out.push_str(URL_PLACEHOLDER);
            i = end;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

fn match_url_at(chars: &[char], i: usize) -> Option<usize> {
    let scheme_len = if starts_with_ci(chars, i, "https://") {
        8
    } else if starts_with_ci(chars, i, "http://") {
        7
    } else if starts_with_ci(chars, i, "www.") {
        4
    } else {
        return None;
    };
    let mut j = i + scheme_len;
    if j >= chars.len() || chars[j].is_whitespace() {
        return None;
    }
    while j < chars.len() && !chars[j].is_whitespace() {
        j += 1;
    }
    // Leave sentence punctuation outside the URL match.
    while j > i + scheme_len {
        let prev = chars[j - 1];
        if matches!(prev, '.' | ',' | ';' | ':' | '!' | '?' | ')' | ']' | '\'' | '"' | '»') {
            j -= 1;
        } else {
            break;
        }
    }
    if j <= i + scheme_len {
        return None;
    }
    Some(j)
}

fn starts_with_ci(chars: &[char], i: usize, prefix: &str) -> bool {
    let p: Vec<char> = prefix.chars().collect();
    if i + p.len() > chars.len() {
        return false;
    }
    p.iter()
        .enumerate()
        .all(|(k, &want)| chars[i + k].eq_ignore_ascii_case(&want))
}

/// Drop whitespace-separated tokens that are pure social hashtags (`#tag`).
fn drop_standalone_hashtags(s: &str) -> String {
    s.split_whitespace()
        .filter(|tok| !is_hashtag_token(tok))
        .collect::<Vec<_>>()
        .join(" ")
}

fn is_hashtag_token(tok: &str) -> bool {
    let mut it = tok.chars();
    match it.next() {
        Some('#') => {
            let rest: String = it.collect();
            !rest.is_empty()
                && rest
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
        }
        _ => false,
    }
}

/// Space-split any token longer than `max_len` so the segmenter never sees a giant blob.
fn cap_unbroken_tokens(s: &str, max_len: usize) -> String {
    let max_len = max_len.max(8);
    let mut parts: Vec<String> = Vec::new();
    for token in s.split_whitespace() {
        let chars: Vec<char> = token.chars().collect();
        if chars.len() <= max_len {
            parts.push(token.to_string());
            continue;
        }
        for chunk in chars.chunks(max_len) {
            parts.push(chunk.iter().collect());
        }
    }
    parts.join(" ")
}

fn is_handle_only(s: &str) -> bool {
    let t = s.trim();
    if !t.starts_with('@') || t.len() < 2 {
        return false;
    }
    t[1..]
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn keep_char(c: char) -> bool {
    if c.is_ascii() {
        return !c.is_control() || c == '\t';
    }
    // Letters (incl. FR accents) and numbers
    if c.is_alphanumeric() {
        return true;
    }
    matches!(c, '’' | '‘' | '“' | '”' | '–' | '—' | '…' | '«' | '»' | '€' | '£')
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(' ');
                prev_space = true;
            }
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

/// Case rules for acronym expansion.
#[derive(Clone, Copy)]
enum AcronymCase {
    /// Expand only when the token is fully uppercase (`EU`, not French `eu`).
    UpperOnly,
    /// Expand case-insensitively (`tts` / `TTS` / `Tts` → spelled out).
    AnyCase,
}

/// Expand initialisms that Chatterbox tends to mispronounce (B25: TTS → "tits").
///
/// Walks **Unicode scalar values** (not raw UTF-8 bytes). Byte-index expansion
/// was corrupting FR accents on the real `do_speak` path (`Café` → `CafÃ©`).
///
/// Product/technical initialisms are intentionally explicit rather than
/// spelling every uppercase word: names such as `ILIAS` must remain words.
/// `EU` is uppercase-only so French lowercase `eu` ("had") is left alone.
fn expand_acronyms(s: &str) -> String {
    /// (ASCII acronym, spoken expansion, case rule). Longer entries first.
    const ACRONYMS: &[(&str, &str, AcronymCase)] = &[
        ("RLHF", "R L H F", AcronymCase::UpperOnly),
        ("VRAM", "V R A M", AcronymCase::UpperOnly),
        ("TTS", "T T S", AcronymCase::AnyCase),
        ("STT", "S T T", AcronymCase::AnyCase),
        ("GPT", "G P T", AcronymCase::AnyCase),
        ("LLM", "L L M", AcronymCase::UpperOnly),
        ("NLP", "N L P", AcronymCase::UpperOnly),
        ("API", "A P I", AcronymCase::UpperOnly),
        ("CPU", "C P U", AcronymCase::UpperOnly),
        ("GPU", "G P U", AcronymCase::UpperOnly),
        ("RAM", "R A M", AcronymCase::UpperOnly),
        ("AI", "A I", AcronymCase::UpperOnly),
        ("ML", "M L", AcronymCase::UpperOnly),
        ("UI", "U I", AcronymCase::UpperOnly),
        ("UX", "U X", AcronymCase::UpperOnly),
        ("EU", "E U", AcronymCase::UpperOnly),
    ];
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 16);
    let mut i = 0;
    while i < chars.len() {
        if let Some((repl, n)) = match_acronym_at(&chars, i, ACRONYMS) {
            out.push_str(repl);
            i += n;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    out
}

/// Word-boundary match of an ASCII acronym at `chars[i]`.
fn match_acronym_at<'a>(
    chars: &[char],
    i: usize,
    acronyms: &'a [(&str, &str, AcronymCase)],
) -> Option<(&'a str, usize)> {
    for &(acr, repl, case_rule) in acronyms {
        let n = acr.chars().count();
        if i + n > chars.len() {
            continue;
        }
        let slice = &chars[i..i + n];
        let matched = match case_rule {
            AcronymCase::UpperOnly => {
                // Token must equal the uppercase form exactly (ASCII).
                acr.chars()
                    .enumerate()
                    .all(|(j, a)| slice[j].is_ascii() && slice[j] == a)
            }
            AcronymCase::AnyCase => acr.chars().enumerate().all(|(j, a)| {
                let c = slice[j];
                c.is_ascii() && a.to_ascii_uppercase() == c.to_ascii_uppercase()
            }),
        };
        if !matched {
            continue;
        }
        // Left/right boundaries are Unicode-aware so accented letters count as word chars.
        if i > 0 && chars[i - 1].is_alphanumeric() {
            continue;
        }
        if i + n < chars.len() && chars[i + n].is_alphanumeric() {
            continue;
        }
        return Some((repl, n));
    }
    None
}

/// Models stay warm after a successful job unless the user unloads or OOM policy fires.
pub fn should_unload_after_successful_job() -> bool {
    false
}

/// Named regression fixtures (for doctor / smokes / unit tests).
pub fn regression_fixtures() -> &'static [(&'static str, &'static str)] {
    &[
        ("b21-social", B21_SOCIAL_FIXTURE),
        ("b25-tts", B25_TTS_FIXTURE),
    ]
}


#[cfg(test)]
mod tests;
