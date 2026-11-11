//! Unicode emoji helpers: `:shortcode:` replacement, search for the picker and jumbo detection.

use emojis::Group;

use crate::markdown::custom_emoji_at;
use crate::util::{find_tick_run, run_len};

/// Replaces `:shortcode:` with the matching emoji (`:smile:` → 😄).
///
/// Unknown codes, inline code, fenced blocks and custom emoji (`<:name:123>`) are left untouched.
pub fn replace_shortcodes(text: &str) -> String {
    if !text.contains(':') {
        return text.to_string();
    }
    let b = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut i = 0;
    let mut tick_fail = [false; 64];
    while i < b.len() {
        match b[i] {
            b'`' => {
                let run = run_len(b, i, b'`');
                let close = if run < tick_fail.len() && !tick_fail[run] {
                    let c = find_tick_run(b, i + run, run);
                    tick_fail[run] = c.is_none();
                    c
                } else {
                    None
                };
                i = close.map_or(i + run, |c| c + run);
            }
            b'<' => match custom_emoji_at(text, i) {
                Some((_, end)) => i = end,
                None => i += 1,
            },
            b':' => {
                let ns = i + 1;
                let mut j = ns;
                while j < b.len()
                    && j - ns <= 40
                    && (b[j].is_ascii_alphanumeric() || matches!(b[j], b'_' | b'+' | b'-'))
                {
                    j += 1;
                }
                if j > ns
                    && b.get(j) == Some(&b':')
                    && let Some(e) = lookup(&text[ns..j])
                {
                    out.push_str(&text[copied..i]);
                    out.push_str(e);
                    i = j + 1;
                    copied = i;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }
    out.push_str(&text[copied..]);
    out
}

fn lookup(name: &str) -> Option<&'static str> {
    let found = if name.bytes().any(|c| c.is_ascii_uppercase()) {
        emojis::get_by_shortcode(&name.to_ascii_lowercase())
    } else {
        emojis::get_by_shortcode(name)
    };
    found.map(|e| e.as_str())
}

/// Shortcode matches for the picker/autocomplete as `(emoji, shortcode)`: exact, then prefix, then
/// substring matches; shorter shortcodes first. An empty query lists the first emoji in picker order.
pub fn search(query: &str, limit: usize) -> Vec<(&'static str, &'static str)> {
    if limit == 0 {
        return Vec::new();
    }
    let q = query
        .trim()
        .trim_matches(':')
        .to_lowercase()
        .replace(' ', "_");
    if q.is_empty() {
        return emojis::iter()
            .filter_map(|e| e.shortcode().map(|s| (e.as_str(), s)))
            .take(limit)
            .collect();
    }
    let mut hits: Vec<(u8, usize, &'static str, &'static str)> = Vec::new();
    for e in emojis::iter() {
        let best = e
            .shortcodes()
            .filter_map(|s| {
                let rank = if s == q {
                    0
                } else if s.starts_with(&q) {
                    1
                } else if s.contains(&q) {
                    2
                } else {
                    return None;
                };
                Some((rank, s.len(), s))
            })
            .min();
        if let Some((rank, len, s)) = best {
            hits.push((rank, len, s, e.as_str()));
        }
    }
    hits.sort_unstable();
    hits.into_iter()
        .take(limit)
        .map(|(_, _, s, e)| (e, s))
        .collect()
}

fn group_name(g: Group) -> &'static str {
    match g {
        Group::SmileysAndEmotion => "Smileys & Emotion",
        Group::PeopleAndBody => "People & Body",
        Group::AnimalsAndNature => "Animals & Nature",
        Group::FoodAndDrink => "Food & Drink",
        Group::TravelAndPlaces => "Travel & Places",
        Group::Activities => "Activities",
        Group::Objects => "Objects",
        Group::Symbols => "Symbols",
        Group::Flags => "Flags",
    }
}

/// Default-tone emoji grouped for a picker, in CLDR order.
pub fn categories() -> Vec<(&'static str, Vec<&'static str>)> {
    Group::iter()
        .map(|g| {
            let list = g
                .emojis()
                .filter(|e| e.skin_tone().is_none_or(|t| t == emojis::SkinTone::Default))
                .map(|e| e.as_str())
                .collect();
            (group_name(g), list)
        })
        .collect()
}

/// True if `content` is only 1..=`max` emoji (Unicode and/or `<:custom:id>`), ignoring whitespace.
pub fn is_emoji_only(content: &str, max: usize) -> bool {
    let mut count = 0;
    let mut i = 0;
    while i < content.len() {
        let rest = &content[i..];
        let Some(c) = rest.chars().next() else { break };
        if c.is_whitespace() {
            i += c.len_utf8();
            continue;
        }
        let len = if c == '<' {
            match custom_emoji_at(content, i) {
                Some((_, end)) => end - i,
                None => return false,
            }
        } else {
            match unicode_emoji_len(rest) {
                Some(l) => l,
                None => return false,
            }
        };
        count += 1;
        if count > max {
            return false;
        }
        i += len;
    }
    count >= 1
}

const ZWJ: char = '\u{200D}';

fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Length of the emoji sequence at the start of `s` (ZWJ, skin tone, keycap and flag aware).
fn unicode_emoji_len(s: &str) -> Option<usize> {
    let first = s.chars().next()?;
    let mut end = first.len_utf8();
    let mut single = true;
    if is_regional_indicator(first)
        && let Some(n) = s[end..]
            .chars()
            .next()
            .filter(|&n| is_regional_indicator(n))
    {
        end += n.len_utf8();
        single = false;
    }
    while let Some(c) = s[end..].chars().next() {
        match c {
            '\u{FE0F}' | '\u{20E3}' | '\u{1F3FB}'..='\u{1F3FF}' | '\u{E0020}'..='\u{E007F}' => {
                end += c.len_utf8();
                single = false;
            }
            ZWJ => match s[end + ZWJ.len_utf8()..].chars().next() {
                Some(n) => {
                    end += ZWJ.len_utf8() + n.len_utf8();
                    single = false;
                }
                None => break,
            },
            _ => break,
        }
    }
    // Text-style symbols (©, ™, digits, ‼ …) only count with an explicit emoji modifier.
    if single
        && (first.is_ascii()
            || matches!(first, '\u{80}'..='\u{FF}' | '\u{2000}'..='\u{2122}' | '\u{2139}'))
    {
        return None;
    }
    emojis::get(&s[..end]).map(|_| end)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcodes_are_replaced() {
        assert_eq!(replace_shortcodes(":smile: hi"), "😄 hi");
        assert_eq!(replace_shortcodes("a :+1: b :thumbsup: c"), "a 👍 b 👍 c");
        assert_eq!(replace_shortcodes(":SMILE:"), "😄");
        assert_eq!(replace_shortcodes("no codes here"), "no codes here");
    }

    #[test]
    fn unknown_codes_and_colons_stay() {
        assert_eq!(
            replace_shortcodes(":definitelynotanemoji: x"),
            ":definitelynotanemoji: x"
        );
        assert_eq!(replace_shortcodes("time 10:30:45 ok"), "time 10:30:45 ok");
        assert_eq!(replace_shortcodes("::smile::"), ":😄:");
        assert_eq!(replace_shortcodes(":smile"), ":smile");
        assert_eq!(replace_shortcodes(":notreal:smile:"), ":notreal😄");
    }

    #[test]
    fn code_is_left_untouched() {
        assert_eq!(replace_shortcodes("`:smile:` :smile:"), "`:smile:` 😄");
        assert_eq!(
            replace_shortcodes("``a :smile: b`` :smile:"),
            "``a :smile: b`` 😄"
        );
        assert_eq!(
            replace_shortcodes("```\n:smile:\n```\n:smile:"),
            "```\n:smile:\n```\n😄"
        );
        assert_eq!(
            replace_shortcodes("unterminated ` :smile:"),
            "unterminated ` 😄"
        );
    }

    #[test]
    fn custom_emoji_is_untouched() {
        assert_eq!(
            replace_shortcodes("<:smile:123> :smile:"),
            "<:smile:123> 😄"
        );
        assert_eq!(replace_shortcodes("<a:smile:123>"), "<a:smile:123>");
    }

    #[test]
    fn search_orders_prefix_before_contains() {
        let r = search("thumbs", 10);
        assert!(r.iter().any(|&(e, s)| e == "👍" && s == "thumbsup"));
        assert!(r.iter().all(|&(_, s)| s.contains("thumbs")));
        let r = search(":smil", 50);
        let first_contains = r.iter().position(|&(_, s)| !s.starts_with("smil"));
        let last_prefix = r.iter().rposition(|&(_, s)| s.starts_with("smil"));
        if let (Some(c), Some(p)) = (first_contains, last_prefix) {
            assert!(
                p < c,
                "prefix matches must precede substring matches: {r:?}"
            );
        }
        assert_eq!(search("smile", 1)[0].1, "smile");
        assert_eq!(search("smile", 3).len(), 3);
        assert!(search("zzzzqqqq", 5).is_empty());
        assert!(search("smile", 0).is_empty());
        assert_eq!(search("", 4).len(), 4);
    }

    #[test]
    fn categories_cover_all_groups() {
        let cats = categories();
        assert_eq!(cats.len(), 9);
        assert_eq!(cats[0].0, "Smileys & Emotion");
        assert!(cats.iter().all(|(_, l)| !l.is_empty()));
        assert!(cats[0].1.contains(&"😀"));
        assert!(cats.iter().any(|(n, l)| *n == "Flags" && l.contains(&"🇩🇪")));
    }

    #[test]
    fn emoji_only_detection() {
        assert!(is_emoji_only("😄", 27));
        assert!(is_emoji_only("  😄 😄\n😄 ", 27));
        assert!(is_emoji_only("👍🏽", 1));
        assert!(is_emoji_only("👨‍👩‍👧‍👦", 1));
        assert!(is_emoji_only("🇩🇪", 1));
        assert!(is_emoji_only("1️⃣", 1));
        assert!(is_emoji_only("<:blob:123>", 1));
        assert!(is_emoji_only("<a:blob:123> 😄", 2));
        assert!(!is_emoji_only("", 5));
        assert!(!is_emoji_only("   ", 5));
        assert!(!is_emoji_only("hi 😄", 5));
        assert!(!is_emoji_only("😄😄😄", 2));
        assert!(!is_emoji_only("123", 5));
        assert!(!is_emoji_only("™ ©", 5));
        assert!(!is_emoji_only("<:x:1>", 5));
        assert!(!is_emoji_only("<@123>", 5));
        assert!(!is_emoji_only("😄\u{200d}", 5));
    }
}
