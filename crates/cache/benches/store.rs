use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use dn_cache::{Store, StoreLimits};
use dn_core::{Dispatch, Id, Message, User};

fn msg(id: u64, channel: u64) -> Message {
    Message {
        id: Id(id),
        channel_id: Id(channel),
        guild_id: Some(Id(1)),
        author: User { id: Id(id % 50), username: "user".into(), ..Default::default() },
        content: "The quick brown fox jumps over the lazy dog".into(),
        ..Default::default()
    }
}

fn guild_create_json(channels: usize, members: usize) -> String {
    let chans: Vec<String> = (0..channels).map(|i| format!(r#"{{"id":"{}","type":0,"name":"c{i}","position":{i}}}"#, 10_000 + i)).collect();
    let mems: Vec<String> =
        (0..members).map(|i| format!(r#"{{"user":{{"id":"{}","username":"m{i}"}},"roles":[]}}"#, 100_000 + i)).collect();
    format!(
        r#"{{"id":"1","name":"Big","roles":[{{"id":"1","name":"@everyone","permissions":"68608"}}],"channels":[{}],"members":[{}]}}"#,
        chans.join(","),
        mems.join(",")
    )
}

fn bench(c: &mut Criterion) {
    let json = guild_create_json(500, 5_000);
    c.bench_function("parse+apply GUILD_CREATE 500ch/5k members", |b| {
        b.iter(|| {
            let mut s = Store::new(StoreLimits::default());
            s.apply(&Dispatch::parse("GUILD_CREATE", black_box(&json)).unwrap());
            black_box(s.guilds.len())
        })
    });

    c.bench_function("live MESSAGE_CREATE x10k into active channel (cap 3000)", |b| {
        b.iter(|| {
            let mut s = Store::new(StoreLimits::default());
            s.set_active_channel(Some(Id(5)));
            s.messages_loaded(Id(5), vec![], None, true);
            for i in 1..=10_000u64 {
                s.apply(&Dispatch::MessageCreate(Box::new(msg(i, 5))));
            }
            black_box(s.total_loaded_messages())
        })
    });

    let mut s = Store::new(StoreLimits::default());
    s.apply(&Dispatch::parse("GUILD_CREATE", &json).unwrap());
    c.bench_function("channel_tree 500 channels", |b| b.iter(|| black_box(s.channel_tree(Id(1)).len())));
    c.bench_function("member_list 5k members", |b| b.iter(|| black_box(s.member_list(Id(1), None).len())));
}

criterion_group!(benches, bench);
criterion_main!(benches);
