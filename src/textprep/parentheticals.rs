//! Convert prose parentheticals to pauses suitable for generative TTS.

/// Keep parenthetical content, but express the delimiters as comma pauses.
/// Attached pairs such as `main()` remain intact so code-ish input is not
/// rewritten as prose.
pub(super) fn normalize_parenthetical_pauses(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 4);
    let mut normalized_stack: Vec<bool> = Vec::new();

    for (i, &ch) in chars.iter().enumerate() {
        match ch {
            '(' => {
                let normalize = i == 0 || chars[i - 1].is_whitespace();
                normalized_stack.push(normalize);
                if !normalize {
                    out.push(ch);
                    continue;
                }
                trim_trailing_spaces(&mut out);
                if out
                    .chars()
                    .last()
                    .is_some_and(|last| !is_pause_or_terminal(last))
                {
                    out.push(',');
                }
                push_one_space(&mut out);
            }
            ')' => {
                if !normalized_stack.pop().unwrap_or(false) {
                    out.push(ch);
                    continue;
                }
                trim_trailing_spaces(&mut out);
                let next = chars[i + 1..].iter().copied().find(|c| !c.is_whitespace());
                if next.is_some_and(|c| !is_pause_or_terminal(c))
                    && out
                        .chars()
                        .last()
                        .is_some_and(|last| !is_pause_or_terminal(last))
                {
                    out.push(',');
                }
                if next.is_some_and(|c| !is_pause_or_terminal(c)) {
                    push_one_space(&mut out);
                }
            }
            _ => out.push(ch),
        }
    }
    collapse_ws(&out)
}

fn trim_trailing_spaces(s: &mut String) {
    while s.ends_with(char::is_whitespace) {
        s.pop();
    }
}

fn push_one_space(s: &mut String) {
    if !s.is_empty() && !s.ends_with(char::is_whitespace) {
        s.push(' ');
    }
}

fn is_pause_or_terminal(c: char) -> bool {
    matches!(c, ',' | ';' | ':' | '.' | '!' | '?' | '…' | '—' | '–')
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut previous_was_space = false;
    for ch in s.chars() {
        if ch.is_whitespace() {
            if !previous_was_space && !out.is_empty() {
                out.push(' ');
                previous_was_space = true;
            }
        } else {
            out.push(ch);
            previous_was_space = false;
        }
    }
    out.trim().to_string()
}
