//! Panic-freedom and complexity checks on hostile input.

use std::time::{Duration, Instant};

use dn_messaging::markdown::{self, Block, Inline, MAX_DEPTH};
use dn_messaging::{compose, emoji, timestamp};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

const TOKENS: &[&str] = &[
    "*",
    "**",
    "***",
    "_",
    "__",
    "~~",
    "~",
    "||",
    "|",
    "`",
    "``",
    "```",
    "````",
    "\n",
    "\n\n",
    "\\",
    "[",
    "]",
    "(",
    ")",
    "<",
    ">",
    "@",
    "#",
    ":",
    "http://",
    "https://",
    "<@",
    "<@!1>",
    "<@&2>",
    "<#3>",
    "<:a:1>",
    "<a:bb:2>",
    "<t:1>",
    "<t:-9:R>",
    "</x y:5>",
    "<https://a.b>",
    "@everyone",
    "@here",
    "a",
    "b",
    "word",
    " ",
    "  ",
    "\t",
    "> ",
    ">>> ",
    "- ",
    "* ",
    "1. ",
    "# ",
    "## ",
    "### ",
    "-# ",
    "](",
    "](https://x.y)",
    "é",
    "😀",
    "\u{200d}",
    "\u{fe0f}",
    "\r",
    "\0",
    "日本語",
    ":smile:",
    ":+1:",
    "\u{1F1E9}\u{1F1EA}",
];

fn random_input(rng: &mut Rng) -> String {
    let n = rng.below(60);
    let mut s = String::new();
    for _ in 0..n {
        s.push_str(TOKENS[rng.below(TOKENS.len())]);
    }
    s
}

fn random_chars(rng: &mut Rng) -> String {
    let n = rng.below(80);
    (0..n)
        .map(|_| match rng.below(4) {
            0 => char::from_u32(rng.below(0x80) as u32).unwrap_or('?'),
            1 => char::from_u32(0x80 + rng.below(0x800) as u32).unwrap_or('?'),
            2 => char::from_u32(0x1F300 + rng.below(0x400) as u32).unwrap_or('?'),
            _ => ['*', '_', '~', '|', '`', '<', '>', '[', ']', '(', ')', '\\', '\n', ' ', '@', '#', ':'][rng.below(17)],
        })
        .collect()
}

fn inline_depth(inlines: &[Inline]) -> usize {
    inlines
        .iter()
        .map(|i| match i {
            Inline::Bold(c) | Inline::Italic(c) | Inline::Underline(c) | Inline::Strike(c) | Inline::Spoiler(c) => 1 + inline_depth(c),
            Inline::Link { text, .. } => 1 + inline_depth(text),
            _ => 0,
        })
        .max()
        .unwrap_or(0)
}

fn block_depth(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .map(|b| match b {
            Block::Paragraph(i) | Block::Subtext(i) | Block::Heading { content: i, .. } => inline_depth(i),
            Block::Quote(c) => 1 + block_depth(c),
            Block::List { items, .. } => 1 + items.iter().map(|it| block_depth(it)).max().unwrap_or(0),
            Block::CodeBlock { .. } => 0,
        })
        .max()
        .unwrap_or(0)
}

fn exercise(src: &str, rng: &mut Rng) {
    let blocks = markdown::parse(src);
    assert!(block_depth(&blocks) <= MAX_DEPTH + 20, "nesting escaped its cap for {src:?}");
    let _ = markdown::to_plain_text(&blocks, &|_| None);
    let _ = markdown::parse_inline(src);
    let _ = emoji::replace_shortcodes(src);
    let _ = emoji::is_emoji_only(src, 10);

    let limit = [4, 7, 24, 40, 100, 2000][rng.below(6)];
    let chunks = compose::split_message(src, limit);
    for c in &chunks {
        assert!(c.encode_utf16().count() <= limit, "chunk over {limit} for {src:?}: {c:?}");
    }

    for cursor in 0..=src.len() + 2 {
        if let Some(tok) = compose::completion_at(src, cursor) {
            let _ = compose::apply_completion(src, &tok, cursor, "<@1>");
        }
    }
}

#[test]
fn fuzz_token_soup_never_panics() {
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    for _ in 0..14_000 {
        let s = random_input(&mut rng);
        exercise(&s, &mut rng);
    }
}

#[test]
fn fuzz_random_unicode_never_panics() {
    let mut rng = Rng(0xD1B5_4A32_D192_ED03);
    for _ in 0..6_000 {
        let s = random_chars(&mut rng);
        exercise(&s, &mut rng);
    }
    for _ in 0..500 {
        let bytes: Vec<u8> = (0..rng.below(200)).map(|_| rng.next() as u8).collect();
        exercise(&String::from_utf8_lossy(&bytes), &mut rng);
    }
}

#[test]
fn fuzz_timestamps_never_panic() {
    let mut rng = Rng(42);
    for _ in 0..2_000 {
        let unix = rng.next() as i64;
        let now = rng.next() as i64;
        for style in ['t', 'T', 'd', 'D', 'f', 'F', 'R', 'x'] {
            let _ = timestamp::format(unix, style, now, rng.below(2) == 0);
        }
    }
}

fn time_limit() -> Duration {
    // Workspace crates build unoptimised under `cargo test`; the 50 ms target applies to release.
    if cfg!(debug_assertions) { Duration::from_millis(1500) } else { Duration::from_millis(50) }
}

fn timed(name: &str, src: &str) {
    let start = Instant::now();
    let blocks = markdown::parse(src);
    let _ = markdown::to_plain_text(&blocks, &|_| None);
    let elapsed = start.elapsed();
    eprintln!("{name}: {} bytes in {elapsed:?}", src.len());
    assert!(elapsed < time_limit(), "{name} took {elapsed:?}");
}

fn mixed_200kb() -> String {
    let unit = "**Heads up** <@123456789012345678> the build is _green_ :tada:\n> quoted `code` https://example.com/a_b_c [guide](https://example.com/g) ||spoiler|| <t:1618953630:R>\n```rust\nfn main() {}\n```\n- one\n  - two **bold**\n1. a\n# Title\n-# small\n";
    unit.repeat(200_000 / unit.len() + 1)
}

#[test]
fn large_mixed_message_parses_quickly() {
    timed("mixed 200KB", &mixed_200kb());
}

#[test]
fn adversarial_inputs_stay_linear() {
    let n = 200_000;
    let cases: Vec<(&str, String)> = vec![
        ("stars open", "*a ".repeat(n / 3)),
        ("stars close", "a* ".repeat(n / 3)),
        ("nested bold", format!("{}x{}", "**".repeat(n / 4), "**".repeat(n / 4))),
        ("mixed delimiters", "*_~~||".repeat(n / 6)),
        ("alternating delimiters", "*a_b~~c||d".repeat(n / 10)),
        ("underscores", "_a".repeat(n / 2)),
        ("spoilers", "||a".repeat(n / 3)),
        ("backticks", "`a".repeat(n / 2)),
        ("double backticks", "``a".repeat(n / 3)),
        ("triple backticks", "```a".repeat(n / 4)),
        ("fence lines", "```x\n".repeat(n / 5)),
        ("brackets", "[".repeat(n)),
        ("open links", "[a](".repeat(n / 4)),
        ("link urls", "[a](https://x.y/".repeat(n / 16)),
        ("closed brackets", "[a]".repeat(n / 3)),
        ("angle", "<".repeat(n)),
        ("angle links", "<https://x".repeat(n / 10)),
        ("mentions", "<@1".repeat(n / 3)),
        ("autolinks", "https://a.b/".repeat(n / 12)),
        ("autolink punctuation", format!("https://a.b{}", ")".repeat(n))),
        ("autolink stars", format!("https://{}", "*".repeat(n))),
        ("autolinks spaced", "http://* ".repeat(n / 9)),
        ("quotes", "> ".repeat(n / 2)),
        ("quote lines", "> a\n".repeat(n / 4)),
        ("quote all", ">>> a\n".repeat(n / 6)),
        ("headings", "# a\n".repeat(n / 4)),
        ("list lines", "- a\n".repeat(n / 4)),
        ("deep lists", (0..400).map(|i| format!("{}- x\n", " ".repeat(i))).collect()),
        ("list dedent", (0..300).map(|i| format!("{}- x\n", " ".repeat(300 - i))).collect()),
        ("escapes", "\\*".repeat(n / 2)),
        ("backslashes", "\\".repeat(n)),
        ("newlines", "\n".repeat(n)),
        ("emoji shortcodes", ":a:b".repeat(n / 4)),
        ("emoji", "😀".repeat(n / 4)),
    ];
    for (name, src) in &cases {
        timed(name, src);
        let start = Instant::now();
        let _ = emoji::replace_shortcodes(src);
        let _ = emoji::is_emoji_only(src, 10);
        let _ = compose::split_message(src, 2000);
        assert!(start.elapsed() < time_limit() * 4, "helpers on {name} took {:?}", start.elapsed());
    }
}
