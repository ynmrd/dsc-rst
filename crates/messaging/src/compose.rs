//! Composer helpers: autocomplete tokens and splitting over-long messages.

use crate::util::{is_lang, utf16_len};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompletionKind {
    User,
    Channel,
    Emoji,
    Role,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionToken {
    pub kind: CompletionKind,
    /// Byte index of the trigger character (`@`, `#` or `:`).
    pub start: usize,
    pub query: String,
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    i = i.min(text.len());
    while !text.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Autocomplete trigger ending at the cursor, if any.
///
/// `@` completes users (roles are resolved by the UI from the same query), `#` channels and `:`
/// emoji (at least two query characters). The trigger must start the text or follow whitespace and
/// the query must not contain whitespace.
pub fn completion_at(text: &str, cursor_byte: usize) -> Option<CompletionToken> {
    let cursor = floor_boundary(text, cursor_byte);
    let before = &text[..cursor];
    let start = before.char_indices().rev().find(|(_, c)| c.is_whitespace()).map_or(0, |(i, c)| i + c.len_utf8());
    let word = &before[start..];
    let trigger = word.chars().next()?;
    let query = &word[trigger.len_utf8()..];
    let kind = match trigger {
        '@' => CompletionKind::User,
        '#' => CompletionKind::Channel,
        ':' => {
            let valid = query.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '+' | '-'));
            if query.chars().count() < 2 || !valid {
                return None;
            }
            CompletionKind::Emoji
        }
        _ => return None,
    };
    Some(CompletionToken { kind, start, query: query.to_string() })
}

/// Replaces the token (from its trigger up to the cursor) with `replacement` plus a trailing space.
/// An existing space after the cursor is reused. Returns the new text and cursor position.
pub fn apply_completion(text: &str, token: &CompletionToken, cursor_byte: usize, replacement: &str) -> (String, usize) {
    let cursor = floor_boundary(text, cursor_byte);
    let start = floor_boundary(text, token.start).min(cursor);
    let after = &text[cursor..];
    let mut out = String::with_capacity(text.len() + replacement.len() + 1);
    out.push_str(&text[..start]);
    out.push_str(replacement);
    out.push(' ');
    let new_cursor = out.len();
    out.push_str(after.strip_prefix(' ').unwrap_or(after));
    (out, new_cursor)
}

const FENCE: &str = "```";
const CLOSE_RESERVE: usize = 4; // "\n```"

/// Splits `content` into chunks of at most `limit` UTF-16 units (Discord's 2000-character limit).
///
/// Cuts prefer line breaks, then spaces, and never fall inside a character. A fenced code block cut
/// by a boundary is closed at the end of its chunk and reopened (with its language) in the next.
/// Limits below 4 are raised to 4; fence repair is skipped below 24. Blank content yields no chunks.
pub fn split_message(content: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(4);
    if content.trim().is_empty() {
        return Vec::new();
    }
    if utf16_len(content) <= limit {
        return vec![content.to_string()];
    }
    let mut sp = Splitter { limit, fences: limit >= 24, chunks: Vec::new(), cur: String::new(), len: 0, has_body: false, fence: None };
    for line in content.split('\n') {
        sp.line(line);
    }
    if sp.has_body {
        sp.chunks.push(std::mem::take(&mut sp.cur));
    }
    sp.chunks
}

struct Splitter {
    limit: usize,
    fences: bool,
    chunks: Vec<String>,
    cur: String,
    len: usize,
    /// Whether `cur` holds anything beyond a reopened fence prefix.
    has_body: bool,
    /// Reopen prefix (```lang\n) while inside a fence at the end of `cur`.
    fence: Option<String>,
}

impl Splitter {
    fn next_fence(&self, line: &str) -> Option<String> {
        if !self.fences || line.matches(FENCE).count() % 2 != 1 {
            return self.fence.clone();
        }
        if self.fence.is_some() {
            return None;
        }
        let tail = line.rfind(FENCE).map_or("", |i| line[i + 3..].trim());
        let lang = if is_lang(tail) { tail } else { "" };
        let prefix = format!("{FENCE}{lang}\n");
        Some(if utf16_len(&prefix) + CLOSE_RESERVE > self.limit / 2 { format!("{FENCE}\n") } else { prefix })
    }

    fn append(&mut self, piece: &str) {
        if self.has_body {
            self.cur.push('\n');
            self.len += 1;
        }
        self.cur.push_str(piece);
        self.len += utf16_len(piece);
        self.has_body = true;
    }

    fn start_chunk(&mut self) {
        self.cur = self.fence.clone().unwrap_or_default();
        self.len = utf16_len(&self.cur);
        self.has_body = false;
    }

    fn flush(&mut self) {
        let mut chunk = std::mem::take(&mut self.cur);
        if self.fence.is_some() {
            chunk.push('\n');
            chunk.push_str(FENCE);
        }
        self.chunks.push(chunk);
        self.start_chunk();
    }

    fn line(&mut self, line: &str) {
        if self.fence.is_none() && !self.has_body && line.trim().is_empty() {
            return;
        }
        let after = self.next_fence(line);
        let reserve_last = if after.is_some() { CLOSE_RESERVE } else { 0 };
        let mut rest = line;
        loop {
            let sep = usize::from(self.has_body);
            if self.len + sep + utf16_len(rest) + reserve_last <= self.limit {
                self.append(rest);
                break;
            }
            if self.has_body {
                self.flush();
                continue;
            }
            let reserve = if self.fence.is_some() { CLOSE_RESERVE } else { 0 };
            let cap = self.limit.saturating_sub(self.len + reserve).max(2);
            let cut = cut_point(rest, cap);
            let piece = rest[..cut].trim_end_matches(' ');
            if !piece.is_empty() {
                self.append(piece);
                self.flush();
            }
            rest = rest[cut..].trim_start_matches(' ');
            if rest.is_empty() {
                break;
            }
        }
        self.fence = after;
    }
}

/// Byte index at which to cut `s` so the head fits `cap` UTF-16 units, preferring a space.
fn cut_point(s: &str, cap: usize) -> usize {
    let mut units = 0;
    let mut end = 0;
    let mut last_space = None;
    for (i, c) in s.char_indices() {
        units += c.len_utf16();
        if units > cap {
            break;
        }
        end = i + c.len_utf8();
        if c == ' ' && i > 0 {
            last_space = Some(i);
        }
    }
    if end == s.len() {
        return end;
    }
    match last_space {
        Some(i) => i,
        None if end == 0 => s.chars().next().map_or(0, char::len_utf8),
        None => end,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tok(text: &str, cursor: usize) -> Option<CompletionToken> {
        completion_at(text, cursor)
    }

    #[test]
    fn completion_kinds() {
        let t = tok("hello @ali", 10).unwrap();
        assert_eq!((t.kind, t.start, t.query.as_str()), (CompletionKind::User, 6, "ali"));
        let t = tok("#gen", 4).unwrap();
        assert_eq!((t.kind, t.start, t.query.as_str()), (CompletionKind::Channel, 0, "gen"));
        let t = tok("go :smi", 7).unwrap();
        assert_eq!((t.kind, t.start, t.query.as_str()), (CompletionKind::Emoji, 3, "smi"));
        assert_eq!(tok("@", 1).unwrap().query, "");
        assert_eq!(tok("a\n@bob", 6).unwrap().start, 2);
    }

    #[test]
    fn completion_rejections() {
        assert!(tok("mail@host", 9).is_none());
        assert!(tok("x#1", 3).is_none());
        assert!(tok("10:30", 5).is_none());
        assert!(tok(":s", 2).is_none());
        assert!(tok(":smile:", 7).is_none());
        assert!(tok(":)", 2).is_none());
        assert!(tok("@alice ", 7).is_none());
        assert!(tok("plain", 5).is_none());
        assert!(tok("", 0).is_none());
    }

    #[test]
    fn completion_uses_text_before_cursor_and_clamps() {
        let t = tok("@alice bob", 4).unwrap();
        assert_eq!(t.query, "ali");
        assert!(tok("@é", 2).is_some_and(|t| t.query.is_empty()));
        assert!(tok("héllo @a", 1000).is_some());
        let t = tok("@éa", 2).unwrap();
        assert_eq!((t.start, t.query.as_str()), (0, ""));
    }

    #[test]
    fn apply_completion_replaces_and_moves_cursor() {
        let t = tok("hi @al", 6).unwrap();
        assert_eq!(apply_completion("hi @al", &t, 6, "<@123>"), ("hi <@123> ".to_string(), 10));
        let t = tok("hi @al there", 6).unwrap();
        assert_eq!(apply_completion("hi @al there", &t, 6, "<@123>"), ("hi <@123> there".to_string(), 10));
        let t = tok(":smi", 4).unwrap();
        assert_eq!(apply_completion(":smi", &t, 4, "😄"), ("😄 ".to_string(), 5));
        let bogus = CompletionToken { kind: CompletionKind::User, start: 99, query: String::new() };
        assert_eq!(apply_completion("ab", &bogus, 2, "x").0, "abx ");
    }

    fn units(s: &str) -> usize {
        utf16_len(s)
    }

    #[test]
    fn short_and_blank_messages() {
        assert_eq!(split_message("hello", 2000), vec!["hello"]);
        assert!(split_message("", 2000).is_empty());
        assert!(split_message("  \n ", 2000).is_empty());
    }

    #[test]
    fn splits_on_lines_then_spaces() {
        let text = (0..50).map(|i| format!("line number {i:02}")).collect::<Vec<_>>().join("\n");
        let chunks = split_message(&text, 100);
        assert!(chunks.len() > 1);
        assert!(chunks.iter().all(|c| units(c) <= 100));
        assert_eq!(chunks.join("\n"), text);

        let words = vec!["word"; 1000].join(" ");
        let chunks = split_message(&words, 2000);
        assert_eq!(chunks.len(), 3);
        assert!(chunks.iter().all(|c| units(c) <= 2000 && !c.starts_with(' ') && !c.ends_with(' ')));
        assert_eq!(chunks.join(" "), words);
    }

    #[test]
    fn hard_cuts_respect_chars_and_limits() {
        let text = "é".repeat(5000);
        let chunks = split_message(&text, 2000);
        assert!(chunks.iter().all(|c| units(c) <= 2000));
        assert_eq!(chunks.concat(), text);
        let emoji = "😀".repeat(1500);
        let chunks = split_message(&emoji, 2000);
        assert!(chunks.iter().all(|c| units(c) <= 2000));
        assert_eq!(chunks.concat(), emoji);
        assert!(split_message(&emoji, 1).iter().all(|c| !c.is_empty()));
    }

    #[test]
    fn fences_are_closed_and_reopened() {
        let body = (0..40).map(|i| format!("let x{i} = {i};")).collect::<Vec<_>>().join("\n");
        let text = format!("intro\n```rust\n{body}\n```\noutro");
        let chunks = split_message(&text, 200);
        assert!(chunks.len() > 2);
        assert!(chunks.iter().all(|c| units(c) <= 200), "{chunks:?}");
        for c in &chunks {
            assert_eq!(c.matches("```").count() % 2, 0, "unbalanced fence in {c:?}");
        }
        assert!(chunks[1..chunks.len() - 1].iter().all(|c| c.starts_with("```rust\n")) || chunks[1].starts_with("```rust\n"));
        assert!(chunks.last().unwrap().ends_with("outro"));
        let code: String = chunks.iter().flat_map(|c| c.lines()).filter(|l| l.starts_with("let ")).collect::<Vec<_>>().join("\n");
        assert_eq!(code, body);
    }

    #[test]
    fn long_single_line_in_fence_stays_within_limit() {
        let text = format!("```\n{}\n```", "x".repeat(500));
        let chunks = split_message(&text, 100);
        assert!(chunks.iter().all(|c| units(c) <= 100));
        assert!(chunks.iter().all(|c| c.matches("```").count() % 2 == 0));
    }
}
