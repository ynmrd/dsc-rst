/// Length in UTF-16 code units, which is what Discord's message limit effectively counts.
pub(crate) fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// Start of the next backtick run of exactly `n` backticks at or after `from`.
pub(crate) fn find_tick_run(b: &[u8], from: usize, n: usize) -> Option<usize> {
    let mut j = from;
    while j < b.len() {
        if b[j] == b'`' {
            let start = j;
            while j < b.len() && b[j] == b'`' {
                j += 1;
            }
            if j - start == n {
                return Some(start);
            }
        } else {
            j += 1;
        }
    }
    None
}

pub(crate) fn run_len(b: &[u8], from: usize, ch: u8) -> usize {
    b[from..].iter().take_while(|&&c| c == ch).count()
}

/// Valid code-fence language tag (single word, no spaces).
pub(crate) fn is_lang(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 32
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'+' | b'-' | b'.' | b'#'))
}
