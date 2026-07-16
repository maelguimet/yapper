//! Sentence / clause segmentation for chunked TTS (EN + FR friendly).

/// Default max characters per synth chunk (runaway lines get hard-split).
pub const DEFAULT_MAX_CHUNK_CHARS: usize = 280;

/// Split text into speakable segments.
///
/// Rules:
/// - Prefer sentence boundaries `.?!…` and French `«»` / `:` pauses when reasonable.
/// - Soft-break on clause punctuation (`,`, `;`, `:`) near the hard limit.
/// - Keep common abbreviations from ending a segment early (Mr., Dr., etc.).
/// - Cap chunk length; hard-split long runs on whitespace when needed.
/// - Non-final hard-split segments without terminal punctuation get a light
///   synth-only period so generative TTS is less likely to invent a tail.
/// - Preserve order; drop empty segments; trim whitespace.
pub fn split_for_tts(text: &str) -> Vec<String> {
    split_for_tts_with_limit(text, DEFAULT_MAX_CHUNK_CHARS)
}

pub fn split_for_tts_with_limit(text: &str, max_chars: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }
    let max_chars = max_chars.max(40);
    let mut segments = Vec::new();
    let mut buf = String::new();

    let chars: Vec<char> = text.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        buf.push(ch);

        let at_end = i + 1 >= chars.len();
        let next_is_space_or_end = at_end
            || chars[i + 1].is_whitespace()
            || matches!(chars[i + 1], '"' | '\'' | '»' | ')' | ']');

        if is_sentence_ender(ch) && next_is_space_or_end && !is_abbreviation(&buf) {
            // Include trailing closers/quotes
            let mut j = i + 1;
            while j < chars.len() && matches!(chars[j], '"' | '\'' | '»' | ')' | ']' | '”' | '’') {
                buf.push(chars[j]);
                j += 1;
            }
            push_segment(&mut segments, &buf, max_chars);
            buf.clear();
            // skip whitespace between sentences
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            i = j;
            continue;
        }

        // Soft break on newline when buffer is non-trivial
        if ch == '\n' && buf.trim().chars().count() >= 20 {
            push_segment(&mut segments, &buf, max_chars);
            buf.clear();
        }

        // Prefer clause boundaries when approaching the hard cap.
        let buf_len = buf.chars().count();
        if buf_len >= max_chars.saturating_mul(3) / 4
            && is_clause_break(ch)
            && next_is_space_or_end
            && buf_len >= 40
        {
            push_segment(&mut segments, &buf, max_chars);
            buf.clear();
            let mut j = i + 1;
            while j < chars.len() && chars[j].is_whitespace() {
                j += 1;
            }
            i = j;
            continue;
        }

        // Hard cap: flush at last whitespace if over limit
        if buf.chars().count() >= max_chars {
            push_segment(&mut segments, &buf, max_chars);
            buf.clear();
        }

        i += 1;
    }
    if !buf.trim().is_empty() {
        push_segment(&mut segments, &buf, max_chars);
    }
    ensure_nonfinal_terminal_punct(&mut segments, max_chars);
    segments
}

fn is_clause_break(ch: char) -> bool {
    matches!(ch, ',' | ';' | ':')
}

/// Append a light period to non-final segments that lack terminal punctuation.
/// Helps generative TTS treat hard-split mid-thought chunks as complete.
/// Skips when the segment is already at `max_chars` so the limit stays hard.
fn ensure_nonfinal_terminal_punct(segments: &mut [String], max_chars: usize) {
    if segments.len() < 2 {
        return;
    }
    let last = segments.len() - 1;
    for seg in segments.iter_mut().take(last) {
        let trimmed = seg.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.chars().count() >= max_chars {
            continue;
        }
        let last_ch = trimmed.chars().last().unwrap_or(' ');
        if is_sentence_ender(last_ch) || is_clause_break(last_ch) {
            continue;
        }
        // Don't double-punctuate closers like quotes after a period already handled.
        if matches!(last_ch, '"' | '\'' | '»' | ')' | ']' | '”' | '’') {
            continue;
        }
        seg.push('.');
    }
}

fn is_sentence_ender(ch: char) -> bool {
    matches!(ch, '.' | '!' | '?' | '…' | '。' | '！' | '？')
}

fn is_abbreviation(buf: &str) -> bool {
    let trimmed = buf.trim_end();
    let lower = trimmed.to_ascii_lowercase();
    // Ends with known abbrev token
    const ABBREVS: &[&str] = &[
        "mr.", "mrs.", "ms.", "dr.", "prof.", "sr.", "jr.", "vs.", "etc.", "e.g.", "i.e.", "st.",
        "rd.", "ave.", "approx.", "dept.", "est.", "fig.", "vol.", "pp.", "no.",
        // French
        "m.", "mme.", "mlle.", "dr.", "me.", "etc.", "cf.", "ex.",
    ];
    for a in ABBREVS {
        if lower.ends_with(a) {
            // Ensure it's a token boundary (start or whitespace before)
            let without = &lower[..lower.len().saturating_sub(a.len())];
            if without.is_empty()
                || without
                    .chars()
                    .last()
                    .map(|c| c.is_whitespace() || c == '(' || c == '«')
                    .unwrap_or(true)
            {
                return true;
            }
        }
    }
    // Single capital letter + period (e.g. "A. Turing" mid-initial). A run of
    // spaced capitals is a spoken initialism (`A I.` / `R L H F.`), whose final
    // period is a real sentence boundary.
    if let Some(stripped) = trimmed.strip_suffix('.') {
        let mut words = stripped.split_whitespace().rev();
        let last = words.next().unwrap_or("");
        let previous = words.next().unwrap_or("");
        if is_single_upper_letter(last) && !is_single_upper_letter(previous) {
            return true;
        }
    }
    false
}

fn is_single_upper_letter(token: &str) -> bool {
    let mut letters = token
        .trim_matches(|c: char| matches!(c, '(' | '[' | '"' | '\'' | '«' | '“'))
        .chars();
    matches!(letters.next(), Some(c) if c.is_uppercase()) && letters.next().is_none()
}

fn push_segment(out: &mut Vec<String>, raw: &str, max_chars: usize) {
    let s = raw.trim();
    if s.is_empty() {
        return;
    }
    if s.chars().count() <= max_chars {
        out.push(s.to_string());
        return;
    }
    // Hard-split long segment on whitespace; giant single tokens are char-chunked.
    let mut part = String::new();
    for word in s.split_whitespace() {
        let word_len = word.chars().count();
        if word_len > max_chars {
            if !part.is_empty() {
                out.push(std::mem::take(&mut part));
            }
            push_hard_split_token(out, word, max_chars);
            continue;
        }
        if part.is_empty() {
            part.push_str(word);
            continue;
        }
        if part.chars().count() + 1 + word_len > max_chars {
            out.push(std::mem::take(&mut part));
            part.push_str(word);
        } else {
            part.push(' ');
            part.push_str(word);
        }
    }
    if !part.is_empty() {
        out.push(part);
    }
}

/// Split one oversize token into pieces of at most `max_chars` (Unicode scalars).
fn push_hard_split_token(out: &mut Vec<String>, word: &str, max_chars: usize) {
    let piece = max_chars.max(1);
    let chars: Vec<char> = word.chars().collect();
    for chunk in chars.chunks(piece) {
        out.push(chunk.iter().collect());
    }
}

/// Estimate how many synth steps a text will take (for status UI).
pub fn estimate_segment_count(text: &str) -> usize {
    split_for_tts(text).len().max(if text.trim().is_empty() { 0 } else { 1 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace() {
        assert!(split_for_tts("").is_empty());
        assert!(split_for_tts("   \n  ").is_empty());
    }

    #[test]
    fn english_multi_sentence() {
        let segs = split_for_tts(
            "Hello world. How are you? I am fine! Let's go… Now we continue.",
        );
        assert!(segs.len() >= 4, "{segs:?}");
        assert!(segs[0].contains("Hello world"));
        assert!(segs.iter().any(|s| s.contains("How are you")));
        assert!(segs.iter().any(|s| s.contains("I am fine")));
    }

    #[test]
    fn french_multi_sentence() {
        let segs = split_for_tts(
            "Bonjour le monde. Comment ça va ? Très bien ! Allons-y. Merci beaucoup.",
        );
        assert!(segs.len() >= 4, "{segs:?}");
        assert!(segs[0].contains("Bonjour"));
        assert!(segs.iter().any(|s| s.contains("Comment")));
        assert!(segs.iter().any(|s| s.contains("Merci")));
    }

    #[test]
    fn abbreviations_do_not_split_early() {
        let segs = split_for_tts("Dr. Smith met Mr. Jones on St. Louis Ave. yesterday.");
        // Prefer fewer segments than one per abbrev period
        assert!(
            segs.len() <= 2,
            "abbreviations should not explode: {segs:?}"
        );
        assert!(segs[0].contains("Dr. Smith") || segs.join(" ").contains("Dr. Smith"));
    }

    #[test]
    fn spelled_initialisms_end_sentences() {
        let segs = split_for_tts("A I. R L H F. Done.");
        assert_eq!(segs, ["A I.", "R L H F.", "Done."]);
    }

    #[test]
    fn parenthetical_initialism_does_not_swallow_next_sentence() {
        let cleaned = crate::textprep::sanitize_for_tts(
            "This uses AI (and RLHF). Parentheticals keep working.",
        );
        let segs = split_for_tts(&cleaned);
        assert_eq!(
            segs,
            [
                "This uses A I, and R L H F.",
                "Parentheticals keep working."
            ]
        );
    }

    #[test]
    fn long_run_hard_split() {
        let word = "syllable";
        let long = format!("{}!", vec![word; 80].join(" "));
        let segs = split_for_tts_with_limit(&long, 100);
        assert!(segs.len() > 1, "{segs:?}");
        for s in &segs {
            assert!(s.chars().count() <= 120, "seg too long: {}", s.chars().count());
        }
    }

    #[test]
    fn single_sentence_one_chunk() {
        let segs = split_for_tts("Just one sentence without drama");
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0], "Just one sentence without drama");
    }

    #[test]
    fn preserves_order_en_fr_mix() {
        let segs = split_for_tts("Hello. Bonjour. Goodbye.");
        assert_eq!(segs.len(), 3);
        assert!(segs[0].starts_with("Hello"));
        assert!(segs[1].starts_with("Bonjour"));
        assert!(segs[2].starts_with("Goodbye"));
    }

    #[test]
    fn estimate_matches_split_len() {
        let t = "One. Two. Three.";
        assert_eq!(estimate_segment_count(t), split_for_tts(t).len());
        assert_eq!(estimate_segment_count(""), 0);
    }

    #[test]
    fn hard_split_nonfinal_gets_terminal_punct() {
        // Long run with no sentence enders — hard split must leave non-final
        // chunks with a period so TTS is less likely to invent a tail.
        let words: Vec<&str> = (0..60).map(|_| "word").collect();
        let long = words.join(" ");
        let segs = split_for_tts_with_limit(&long, 80);
        assert!(segs.len() > 1, "{segs:?}");
        for (i, s) in segs.iter().enumerate() {
            if i + 1 < segs.len() {
                let last = s.chars().last().unwrap_or(' ');
                assert!(
                    is_sentence_ender(last) || is_clause_break(last),
                    "non-final seg {i} lacks terminal punct: {s:?}"
                );
            }
        }
    }

    #[test]
    fn prefers_clause_break_before_hard_limit() {
        let text = format!(
            "{}, and then more words keep going past the limit without a period so we split",
            "clause phrase".repeat(8)
        );
        let segs = split_for_tts_with_limit(&text, 100);
        assert!(segs.len() >= 2, "{segs:?}");
    }

    #[test]
    fn oversize_single_token_hard_split_under_limit() {
        let blob: String = std::iter::repeat('x').take(1000).collect();
        let max = 80;
        let segs = split_for_tts_with_limit(&blob, max);
        assert!(segs.len() > 1, "must hard-split: {segs:?}");
        for s in &segs {
            assert!(
                s.chars().count() <= max,
                "seg len {} > {max}: {:?}",
                s.chars().count(),
                s
            );
        }
        let rejoined: String = segs.join("");
        // Non-final hard-split may append a light period; strip for body check.
        let body: String = rejoined.chars().filter(|c| *c == 'x').collect();
        assert_eq!(body.len(), 1000);
    }

    #[test]
    fn sanitize_then_split_keeps_segments_under_chunk_limit() {
        use crate::textprep::sanitize_for_tts;
        let blob: String = std::iter::repeat('z').take(1000).collect();
        let messy = format!(
            "Read https://very.long.example.com/path/{} and #spam {}",
            "p".repeat(200),
            blob
        );
        let cleaned = sanitize_for_tts(&messy);
        let max = DEFAULT_MAX_CHUNK_CHARS;
        let segs = split_for_tts_with_limit(&cleaned, max);
        assert!(!segs.is_empty());
        for s in &segs {
            assert!(
                s.chars().count() <= max,
                "seg len {} > {max}: {:?}",
                s.chars().count(),
                &s[..s.len().min(40)]
            );
        }
        assert!(!cleaned.contains("http"), "URL stripped in sanitize: {cleaned}");
    }
}
