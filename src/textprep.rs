//! Sanitize and normalize free-form text before TTS (social/X paste, acronyms).

/// Exact B21 fixture (social/X paste that must not crash the pipeline).
pub const B21_SOCIAL_FIXTURE: &str = "\
ILIAS ISM
@illyism
This doesn't work in EU btw 🫡

Neither the GPT-Live from today

The permanent EUnderclass is already here 🇪🇺";

/// B25 pronunciation fixture.
pub const B25_TTS_FIXTURE: &str = "This is a test. Does the TTS work?";

/// Sanitize messy social/X text for Chatterbox: strip handles, drop unsupported
/// glyphs, collapse blank lines, expand a few acronyms so they are not misread.
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
        if keep_char(c) {
            s.push(c);
        }
    }
    let s = collapse_ws(&s);
    expand_acronyms(&s)
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

/// Expand acronyms that Chatterbox tends to mispronounce (B25: TTS → "tits").
///
/// Walks **Unicode scalar values** (not raw UTF-8 bytes). Byte-index expansion
/// was corrupting FR accents on the real `do_speak` path (`Café` → `CafÃ©`).
///
/// `EU` is uppercase-only so French lowercase `eu` ("had") is left alone.
fn expand_acronyms(s: &str) -> String {
    /// (ASCII acronym, spoken expansion, case rule). Longer entries first.
    const ACRONYMS: &[(&str, &str, AcronymCase)] = &[
        ("TTS", "T T S", AcronymCase::AnyCase),
        ("STT", "S T T", AcronymCase::AnyCase),
        ("GPT", "G P T", AcronymCase::AnyCase),
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
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn b21_social_does_not_panic_and_is_speakable() {
        let out = sanitize_for_tts(B21_SOCIAL_FIXTURE);
        assert!(!out.is_empty(), "sanitized text should remain speakable");
        assert!(!out.contains('@'), "handles stripped: {out}");
        assert!(!out.contains('🫡'), "emoji stripped: {out}");
        assert!(
            out.to_ascii_lowercase().contains("doesn")
                || out.to_ascii_lowercase().contains("work"),
            "{out}"
        );
        assert!(out.contains("G P T") || out.to_ascii_lowercase().contains("g p t"), "{out}");
        assert_eq!(sanitize_for_tts(""), "");
        assert_eq!(sanitize_for_tts("   \n\n  "), "");
    }

    #[test]
    fn b25_tts_expanded_not_vulgar() {
        let out = sanitize_for_tts(B25_TTS_FIXTURE);
        assert!(
            out.contains("T T S"),
            "TTS must be spelled out for pronunciation: {out}"
        );
        assert!(
            !out.split_whitespace().any(|w| w.eq_ignore_ascii_case("TTS")),
            "raw TTS token must not remain: {out}"
        );
        assert!(!out.to_ascii_lowercase().contains("river"));
    }

    #[test]
    fn sticky_load_policy_default() {
        assert!(!should_unload_after_successful_job());
    }

    #[test]
    fn strips_handle_only_lines() {
        let out = sanitize_for_tts("@illyism\nHello there");
        assert_eq!(out, "Hello there");
    }

    #[test]
    fn keeps_french_letters() {
        // Real do_speak path: sanitize must preserve FR accents, not UTF-8 mojibake.
        let out = sanitize_for_tts("Café déjà vu — très bien. ça va?");
        assert!(
            out.contains("Café"),
            "must keep intact Café (not CafÃ©), got {out:?}"
        );
        assert!(out.contains('é'), "must keep é, got {out:?}");
        assert!(out.contains('ç'), "must keep ç, got {out:?}");
        assert!(out.contains("déjà") || out.contains("deja"), "got {out:?}");
        assert!(out.contains("bien"), "{out}");
        // Mojibake fingerprints from byte-as-char expansion (pre-fix bug).
        assert!(
            !out.contains('Ã'),
            "UTF-8 mojibake (Ã) must not appear: {out:?}"
        );
        assert!(
            !out.contains("CafÃ"),
            "classic Café→CafÃ© corruption: {out:?}"
        );
    }

    #[test]
    fn acronym_expand_does_not_corrupt_surrounding_unicode() {
        // Regression: expand_acronyms used to walk bytes and cast to char.
        let out = sanitize_for_tts("Café TTS déjà");
        assert!(out.contains("Café"), "got {out:?}");
        assert!(out.contains("T T S"), "got {out:?}");
        assert!(out.contains('é'), "got {out:?}");
        assert!(!out.contains('Ã'), "got {out:?}");
        // Mixed: FR word + EU acronym boundary
        let out2 = sanitize_for_tts("français EU ok");
        assert!(out2.contains('ç') || out2.contains("fran"), "got {out2:?}");
        assert!(out2.contains("E U"), "got {out2:?}");
        assert!(!out2.contains('Ã'), "got {out2:?}");
    }

    #[test]
    fn french_lowercase_eu_not_expanded() {
        // French "eu" (past of avoir) must not become "E U".
        let out = sanitize_for_tts("j'ai eu peur.");
        assert_eq!(out, "j'ai eu peur.");
        assert!(!out.contains("E U"), "lowercase eu must stay: {out}");
        let out2 = sanitize_for_tts("Bonjour. J'ai eu peur, mais maintenant ça va.");
        assert!(out2.contains("eu"), "got {out2:?}");
        assert!(!out2.contains("E U"), "got {out2:?}");
    }

    #[test]
    fn uppercase_eu_still_expanded() {
        let out = sanitize_for_tts("EU rules");
        assert!(out.contains("E U"), "got {out:?}");
        assert!(!out.split_whitespace().any(|w| w == "EU"), "got {out:?}");
    }

    #[test]
    fn lowercase_tts_stt_gpt_still_expanded() {
        let out = sanitize_for_tts("try tts and stt with gpt");
        assert!(out.contains("T T S"), "got {out:?}");
        assert!(out.contains("S T T"), "got {out:?}");
        assert!(out.contains("G P T"), "got {out:?}");
    }

    #[test]
    fn golden_sanitize_fixtures_match_expected() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("fixtures/sanitize");
        let cases = std::fs::read_to_string(root.join("cases.txt")).expect("cases.txt");
        let expected = std::fs::read_to_string(root.join("expected_rust.txt"))
            .or_else(|_| std::fs::read_to_string(root.join("expected.txt")))
            .expect("expected fixtures");
        let inputs: Vec<&str> = cases.split("\n---\n").collect();
        let wants: Vec<&str> = expected.split("\n---\n").collect();
        assert_eq!(inputs.len(), wants.len(), "fixture count mismatch");
        for (raw, want) in inputs.iter().zip(wants.iter()) {
            assert_eq!(
                sanitize_for_tts(raw),
                want.trim(),
                "input={raw:?}"
            );
        }
    }
}
