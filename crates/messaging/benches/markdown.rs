use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use dn_messaging::{compose, emoji, markdown};

const SHORT: &str = "lol that's hilarious, see you at 8";
const RICH: &str = "**Heads up** <@123456789012345678>: the build is _green_ again :tada:\n> quoted reply with `inline code`\nCheck https://example.com/docs/a_b_c and [the guide](https://example.com/guide) ||spoiler|| <t:1618953630:R>";
const CODE: &str = "Here is the fix:\n```rust\nfn main() {\n    println!(\"hello\");\n}\n```\nLet me know if it works - **thanks**!";
const LIST: &str = "# Plan\n- one\n  - nested **bold**\n- two\n1. first\n2. second\n-# footnote";

fn big_input() -> String {
    let mut s = String::new();
    while s.len() < 200_000 {
        s.push_str(RICH);
        s.push('\n');
        s.push_str(CODE);
        s.push('\n');
        s.push_str(LIST);
        s.push('\n');
    }
    s
}

fn benches(c: &mut Criterion) {
    let mut g = c.benchmark_group("markdown");
    g.bench_function("parse_short", |b| b.iter(|| markdown::parse(black_box(SHORT))));
    g.bench_function("parse_rich", |b| b.iter(|| markdown::parse(black_box(RICH))));
    g.bench_function("parse_code_block", |b| b.iter(|| markdown::parse(black_box(CODE))));
    g.bench_function("parse_lists", |b| b.iter(|| markdown::parse(black_box(LIST))));
    g.bench_function("parse_inline_title", |b| b.iter(|| markdown::parse_inline(black_box("**Build** #42 _passed_"))));
    let blocks = markdown::parse(RICH);
    g.bench_function("plain_text_rich", |b| b.iter(|| markdown::to_plain_text(black_box(&blocks), &|_| None)));
    let big = big_input();
    g.sample_size(20);
    g.bench_function("parse_200kb", |b| b.iter(|| markdown::parse(black_box(&big))));
    g.finish();

    let mut g = c.benchmark_group("helpers");
    g.bench_function("replace_shortcodes", |b| b.iter(|| emoji::replace_shortcodes(black_box("nice :tada: work :+1: `:code:` :unknown:"))));
    g.bench_function("emoji_search", |b| b.iter(|| emoji::search(black_box("smi"), 20)));
    g.bench_function("completion_at", |b| b.iter(|| compose::completion_at(black_box("hello world @ali"), 16)));
    let long = "word ".repeat(2000);
    g.bench_function("split_message_10kb", |b| b.iter(|| compose::split_message(black_box(&long), 2000)));
    g.finish();
}

criterion_group!(group, benches);
criterion_main!(group);
