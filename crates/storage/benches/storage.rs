use std::hint::black_box;

use criterion::{BatchSize, Criterion, Throughput, criterion_group, criterion_main};
use dn_core::{DISCORD_EPOCH_MS, Id, Message, User};
use dn_storage::{SearchFilter, Storage};

const TOTAL: usize = 50_000;
const BATCH: usize = 100;
const CHANNELS: u64 = 40;
const AUTHORS: u64 = 300;

const WORDS: &[&str] = &[
    "the", "quick", "brown", "fox", "jumps", "over", "lazy", "dog", "voice", "channel", "server", "message", "gateway", "connection",
    "audio", "microphone", "latency", "rust", "native", "client", "window", "render", "scroll", "memory", "thread", "async", "tokio",
    "socket", "packet", "opus", "stream", "release", "update", "patch", "build", "deploy", "review", "merge", "branch", "commit",
    "lunch", "meeting", "tomorrow", "weekend", "game", "match", "ranked", "queue", "lobby", "party", "stream", "emoji", "reaction",
    "thanks", "please", "sorry", "great", "awesome", "problem", "solution", "question", "answer", "link", "image", "video", "file",
];

/// Deterministic pseudo-random chat history.
fn history(n: usize) -> Vec<Message> {
    let mut state = 0x2545_f491_4f6c_dd1du64;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    let start_ms = 1_700_000_000_000u64 - DISCORD_EPOCH_MS;
    (0..n as u64)
        .map(|i| {
            let words = 4 + next() % 20;
            let content = (0..words).map(|_| WORDS[(next() % WORDS.len() as u64) as usize]).collect::<Vec<_>>().join(" ");
            let author = next() % AUTHORS;
            Message {
                id: Id(((start_ms + i * 1500) << 22) | (i & 0xfff)),
                channel_id: Id(1000 + next() % CHANNELS),
                guild_id: Some(Id(1)),
                author: User { id: Id(5000 + author), username: format!("user{author}"), ..Default::default() },
                content,
                timestamp: "2023-11-14T22:13:20.000000+00:00".into(),
                ..Default::default()
            }
        })
        .collect()
}

fn filled(msgs: &[Message]) -> (tempfile::TempDir, Storage) {
    let dir = tempfile::tempdir().unwrap();
    let mut s = Storage::open(&dir.path().join("bench.sqlite3")).unwrap();
    for chunk in msgs.chunks(BATCH) {
        s.upsert_messages(chunk).unwrap();
    }
    (dir, s)
}

fn insert(c: &mut Criterion) {
    let msgs = history(TOTAL);
    let mut group = c.benchmark_group("insert");
    group.sample_size(10).throughput(Throughput::Elements(TOTAL as u64));
    group.bench_function("50k_messages_in_batches_of_100", |b| {
        b.iter_batched(
            || {
                let dir = tempfile::tempdir().unwrap();
                let s = Storage::open(&dir.path().join("bench.sqlite3")).unwrap();
                (dir, s)
            },
            |(dir, mut s)| {
                for chunk in msgs.chunks(BATCH) {
                    s.upsert_messages(chunk).unwrap();
                }
                (dir, s)
            },
            BatchSize::PerIteration,
        );
    });
    group.finish();
}

fn search(c: &mut Criterion) {
    let msgs = history(TOTAL);
    let (_dir, s) = filled(&msgs);
    let latest_channel = msgs.last().unwrap().channel_id;
    let query = |q: &str| SearchFilter { query: q.into(), limit: 50, ..Default::default() };

    let mut group = c.benchmark_group("search_50k");
    group.bench_function("common_word", |b| b.iter(|| black_box(s.search(&query("the")).unwrap())));
    group.bench_function("prefix_3_chars", |b| b.iter(|| black_box(s.search(&query("con")).unwrap())));
    group.bench_function("two_terms", |b| b.iter(|| black_box(s.search(&query("voice latency")).unwrap())));
    group.bench_function("three_terms_prefix", |b| b.iter(|| black_box(s.search(&query("rust nat cli")).unwrap())));
    group.bench_function("no_match", |b| b.iter(|| black_box(s.search(&query("zzzzqqq")).unwrap())));
    group.bench_function("common_word_in_channel", |b| {
        let f = SearchFilter { channel_id: Some(latest_channel), ..query("the") };
        b.iter(|| black_box(s.search(&f).unwrap()))
    });
    group.bench_function("common_word_by_author", |b| {
        let f = SearchFilter { author_id: Some(Id(5007)), ..query("the") };
        b.iter(|| black_box(s.search(&f).unwrap()))
    });
    group.bench_function("filter_only_channel", |b| {
        let f = SearchFilter { channel_id: Some(latest_channel), limit: 50, ..Default::default() };
        b.iter(|| black_box(s.search(&f).unwrap()))
    });
    group.finish();

    let mut group = c.benchmark_group("load_50k");
    group.bench_function("newest_50_in_channel", |b| b.iter(|| black_box(s.load_messages(latest_channel, None, 50).unwrap())));
    let mid = msgs[TOTAL / 2].id;
    group.bench_function("page_50_before_mid", |b| b.iter(|| black_box(s.load_messages(latest_channel, Some(mid), 50).unwrap())));
    group.bench_function("latest_message_id", |b| b.iter(|| black_box(s.latest_message_id(latest_channel).unwrap())));
    group.finish();
}

fn prune(c: &mut Criterion) {
    let msgs = history(TOTAL);
    let mut group = c.benchmark_group("prune");
    group.sample_size(10);
    group.bench_function("50k_to_25k_total", |b| {
        b.iter_batched(|| filled(&msgs), |(dir, mut s)| {
            black_box(s.prune(TOTAL / 2, usize::MAX).unwrap());
            (dir, s)
        }, BatchSize::PerIteration);
    });
    group.finish();
}

criterion_group!(benches, insert, search, prune);
criterion_main!(benches);
