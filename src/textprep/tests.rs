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
    // Prefer single shared expected.txt; dual files must match when present.
    let expected = std::fs::read_to_string(root.join("expected.txt"))
        .or_else(|_| std::fs::read_to_string(root.join("expected_rust.txt")))
        .expect("expected fixtures");
    let rust_side = root.join("expected_rust.txt");
    let py_side = root.join("expected_python.txt");
    if rust_side.is_file() && py_side.is_file() {
        let a = std::fs::read_to_string(&rust_side).unwrap();
        let b = std::fs::read_to_string(&py_side).unwrap();
        assert_eq!(a, b, "expected_rust.txt and expected_python.txt must be byte-identical");
    }
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

#[test]
fn urls_become_short_speakable_placeholder() {
    let out = sanitize_for_tts("See https://example.com/path?q=1 now");
    assert!(out.contains("link"), "got {out:?}");
    assert!(!out.contains("http"), "URL must not remain: {out:?}");
    assert!(!out.contains("example.com"), "got {out:?}");
    let out2 = sanitize_for_tts("also WWW.Example.ORG/foo and http://a.co/x");
    assert!(out2.contains("link"), "got {out2:?}");
    assert!(!out2.to_ascii_lowercase().contains("http"), "got {out2:?}");
    assert!(!out2.to_ascii_lowercase().contains("www."), "got {out2:?}");
}

#[test]
fn standalone_hashtags_dropped_prose_kept() {
    let out = sanitize_for_tts("Love rust #cool #stuff today");
    assert_eq!(out, "Love rust today");
    // C# is not a leading-hash hashtag token.
    let out2 = sanitize_for_tts("I use C# daily");
    assert!(out2.contains("C#") || out2.contains("C"), "got {out2:?}");
}

#[test]
fn long_unbroken_token_capped() {
    let blob: String = std::iter::repeat('q').take(1000).collect();
    let out = sanitize_for_tts(&format!("before {blob} after"));
    assert!(out.starts_with("before "), "got {out:?}");
    assert!(out.ends_with(" after"), "got {out:?}");
    for tok in out.split_whitespace() {
        assert!(
            tok.chars().count() <= MAX_UNBROKEN_TOKEN_CHARS,
            "token len {} > {}: {tok:?}…",
            tok.chars().count(),
            MAX_UNBROKEN_TOKEN_CHARS
        );
    }
    // Cap inserts spaces; total blob char count preserved.
    let q_count = out.chars().filter(|c| *c == 'q').count();
    assert_eq!(q_count, 1000, "must not drop body chars");
}

#[test]
fn code_ish_paste_is_speakable() {
    let raw = "fn main() { println!(\"hi\"); } https://github.com/foo/bar.git #rust";
    let out = sanitize_for_tts(raw);
    assert!(out.contains("fn") || out.contains("main"), "got {out:?}");
    assert!(out.contains("link"), "URL → link: {out:?}");
    assert!(!out.contains("http"), "got {out:?}");
    assert!(!out.contains("#rust"), "hashtag dropped: {out:?}");
    for tok in out.split_whitespace() {
        assert!(tok.chars().count() <= MAX_UNBROKEN_TOKEN_CHARS, "{tok}");
    }
}
