use dn_core::Id;

use super::*;

fn t(s: &str) -> Inline {
    Inline::Text(s.to_string())
}

fn inl(s: &str) -> Vec<Inline> {
    parse_inline(s)
}

fn para(s: &str) -> Vec<Block> {
    vec![Block::Paragraph(inl(s))]
}

fn plain(src: &str) -> String {
    to_plain_text(&parse(src), &|_| None)
}

#[test]
fn plain_text_and_empty() {
    assert_eq!(inl("hello world"), vec![t("hello world")]);
    assert_eq!(parse("hello"), vec![Block::Paragraph(vec![t("hello")])]);
    assert!(parse("").is_empty());
    assert!(parse("  \n\n ").is_empty());
    assert!(inl("").is_empty());
    assert_eq!(inl("héllo 😄 wörld"), vec![t("héllo 😄 wörld")]);
}

#[test]
fn bold_italic_underline_strike_spoiler() {
    assert_eq!(inl("**bold**"), vec![Inline::Bold(vec![t("bold")])]);
    assert_eq!(inl("*it*"), vec![Inline::Italic(vec![t("it")])]);
    assert_eq!(inl("_it_"), vec![Inline::Italic(vec![t("it")])]);
    assert_eq!(inl("__under__"), vec![Inline::Underline(vec![t("under")])]);
    assert_eq!(inl("~~gone~~"), vec![Inline::Strike(vec![t("gone")])]);
    assert_eq!(inl("||secret||"), vec![Inline::Spoiler(vec![t("secret")])]);
    assert_eq!(inl("a **b** c"), vec![t("a "), Inline::Bold(vec![t("b")]), t(" c")]);
}

#[test]
fn bold_italic_combination() {
    assert_eq!(inl("***both***"), vec![Inline::Italic(vec![Inline::Bold(vec![t("both")])])]);
    assert_eq!(inl("___both___"), vec![Inline::Italic(vec![Inline::Underline(vec![t("both")])])]);
}

#[test]
fn mixed_nesting() {
    assert_eq!(
        inl("**bold _italic_ ~~strike~~**"),
        vec![Inline::Bold(vec![t("bold "), Inline::Italic(vec![t("italic")]), t(" "), Inline::Strike(vec![t("strike")])])]
    );
    assert_eq!(inl("__*a* **b**__"), vec![Inline::Underline(vec![Inline::Italic(vec![t("a")]), t(" "), Inline::Bold(vec![t("b")])])]);
    assert_eq!(inl("||**x**||"), vec![Inline::Spoiler(vec![Inline::Bold(vec![t("x")])])]);
}

#[test]
fn unbalanced_markers_stay_literal() {
    assert_eq!(inl("**bold"), vec![t("**bold")]);
    assert_eq!(inl("bold**"), vec![t("bold**")]);
    assert_eq!(inl("*a **b* c"), vec![t("*a *"), Inline::Italic(vec![t("b")]), t(" c")]);
    assert_eq!(inl("*foo**bar*"), vec![Inline::Italic(vec![t("foo**bar")])]);
    assert_eq!(inl("**a*"), vec![t("*"), Inline::Italic(vec![t("a")])]);
    assert_eq!(inl("~~oops"), vec![t("~~oops")]);
    assert_eq!(inl("||oops"), vec![t("||oops")]);
    assert_eq!(inl("~one~"), vec![t("~one~")]);
    assert_eq!(inl("|one|"), vec![t("|one|")]);
}

#[test]
fn asterisks_with_spaces_are_literal() {
    assert_eq!(inl("2 * 3 * 4"), vec![t("2 * 3 * 4")]);
    assert_eq!(inl("a * b"), vec![t("a * b")]);
    assert_eq!(inl("* not italic *"), vec![t("* not italic *")]);
}

#[test]
fn underscores_only_at_word_boundaries() {
    assert_eq!(inl("snake_case_word"), vec![t("snake_case_word")]);
    assert_eq!(inl("some_var and other_var"), vec![t("some_var and other_var")]);
    assert_eq!(inl("(_it_)"), vec![t("("), Inline::Italic(vec![t("it")]), t(")")]);
    assert_eq!(inl("_a_b_"), vec![Inline::Italic(vec![t("a_b")])]);
}

#[test]
fn urls_are_not_formatted() {
    assert_eq!(inl("https://a.com/a_b_c"), vec![Inline::AutoLink("https://a.com/a_b_c".into())]);
    assert_eq!(
        inl("see https://a.com/a_b_c and _it_"),
        vec![t("see "), Inline::AutoLink("https://a.com/a_b_c".into()), t(" and "), Inline::Italic(vec![t("it")])]
    );
    assert_eq!(inl("https://a.com/*x*/y"), vec![Inline::AutoLink("https://a.com/*x*/y".into())]);
}

#[test]
fn autolinks_trim_trailing_punctuation() {
    let link = |u: &str| Inline::AutoLink(u.into());
    assert_eq!(inl("go to https://a.com."), vec![t("go to "), link("https://a.com"), t(".")]);
    assert_eq!(inl("(https://a.com/x)"), vec![t("("), link("https://a.com/x"), t(")")]);
    assert_eq!(inl("https://en.wikipedia.org/wiki/Rust_(language)"), vec![link("https://en.wikipedia.org/wiki/Rust_(language)")]);
    assert_eq!(inl("https://a.com/x, ok"), vec![link("https://a.com/x"), t(", ok")]);
    assert_eq!(inl("https://a.com?q=1&b=2!"), vec![link("https://a.com?q=1&b=2"), t("!")]);
    assert_eq!(inl("**https://a.com**"), vec![Inline::Bold(vec![link("https://a.com")])]);
    assert_eq!(inl("||https://a.com||"), vec![Inline::Spoiler(vec![link("https://a.com")])]);
    assert_eq!(inl("http://x.y/path?a=b#frag"), vec![link("http://x.y/path?a=b#frag")]);
    assert_eq!(inl("http://"), vec![t("http://")]);
    assert_eq!(inl("ftp://a.com"), vec![t("ftp://a.com")]);
}

#[test]
fn angle_autolink_suppresses_embeds() {
    assert_eq!(inl("<https://a.com/x_y>"), vec![Inline::AutoLink("https://a.com/x_y".into())]);
    assert_eq!(inl("<ftp://a.com>"), vec![t("<ftp://a.com>")]);
    assert_eq!(inl("<https://a.com"), vec![t("<"), Inline::AutoLink("https://a.com".into())]);
}

#[test]
fn masked_links() {
    assert_eq!(inl("[text](https://a.com)"), vec![Inline::Link { text: vec![t("text")], url: "https://a.com".into() }]);
    assert_eq!(
        inl("[**bold** link](http://a.com/x_y_z)"),
        vec![Inline::Link { text: vec![Inline::Bold(vec![t("bold")]), t(" link")], url: "http://a.com/x_y_z".into() }]
    );
    assert_eq!(
        inl("[a](<https://a.com>) and [b](https://b.com \"title\")"),
        vec![
            Inline::Link { text: vec![t("a")], url: "https://a.com".into() },
            t(" and "),
            Inline::Link { text: vec![t("b")], url: "https://b.com".into() }
        ]
    );
    assert_eq!(
        inl("[w](https://en.wikipedia.org/wiki/Rust_(language))"),
        vec![Inline::Link { text: vec![t("w")], url: "https://en.wikipedia.org/wiki/Rust_(language)".into() }]
    );
    assert_eq!(inl("**[a](https://a.com)**"), vec![Inline::Bold(vec![Inline::Link { text: vec![t("a")], url: "https://a.com".into() }])]);
}

#[test]
fn masked_links_reject_other_schemes_and_malformed() {
    assert_eq!(inl("[x](javascript:alert(1))"), vec![t("[x](javascript:alert(1))")]);
    assert_eq!(inl("[x](ftp://a.com)"), vec![t("[x](ftp://a.com)")]);
    assert_eq!(inl("[x](https://a.com"), vec![t("[x]("), Inline::AutoLink("https://a.com".into())]);
    assert_eq!(inl("[x] (https://a.com)"), vec![t("[x] ("), Inline::AutoLink("https://a.com".into()), t(")")]);
    assert_eq!(inl("[](https://a.com)"), vec![t("[]("), Inline::AutoLink("https://a.com".into()), t(")")]);
    assert_eq!(inl("[just brackets]"), vec![t("[just brackets]")]);
    // Links do not nest: the inner URL in link text stays plain text.
    assert_eq!(inl("[https://a.com](https://b.com)"), vec![Inline::Link { text: vec![t("https://a.com")], url: "https://b.com".into() }]);
}

#[test]
fn inline_code() {
    assert_eq!(inl("`code`"), vec![Inline::Code("code".into())]);
    assert_eq!(inl("a `**x**` b"), vec![t("a "), Inline::Code("**x**".into()), t(" b")]);
    assert_eq!(inl("`` a ` b ``"), vec![Inline::Code("a ` b".into())]);
    assert_eq!(inl("`a` and `b`"), vec![Inline::Code("a".into()), t(" and "), Inline::Code("b".into())]);
    assert_eq!(inl("`unterminated"), vec![t("`unterminated")]);
    assert_eq!(inl("`` x `"), vec![t("`` x `")]);
    assert_eq!(inl("`<@1> _x_ https://a.com`"), vec![Inline::Code("<@1> _x_ https://a.com".into())]);
    assert_eq!(inl("**`x`**"), vec![Inline::Bold(vec![Inline::Code("x".into())])]);
}

#[test]
fn escapes() {
    assert_eq!(inl(r"\*not italic\*"), vec![t("*not italic*")]);
    assert_eq!(inl(r"\*\*not bold\*\*"), vec![t("**not bold**")]);
    assert_eq!(inl(r"\_x\_ \~ \| \` \\"), vec![t("_x_ ~ | ` \\")]);
    assert_eq!(inl(r"a\b"), vec![t(r"a\b")]);
    assert_eq!(inl(r"\<@123>"), vec![t("<@123>")]);
    assert_eq!(inl(r"\[a](https://b.com)"), vec![t("[a]("), Inline::AutoLink("https://b.com".into()), t(")")]);
    assert_eq!(inl("trailing\\"), vec![t("trailing\\")]);
    assert_eq!(inl(r"**a\*b**"), vec![Inline::Bold(vec![t("a*b")])]);
}

#[test]
fn mentions_and_broadcasts() {
    assert_eq!(inl("<@123>"), vec![Inline::UserMention(Id(123))]);
    assert_eq!(inl("<@!123>"), vec![Inline::UserMention(Id(123))]);
    assert_eq!(inl("<@&456>"), vec![Inline::RoleMention(Id(456))]);
    assert_eq!(inl("<#789>"), vec![Inline::ChannelMention(Id(789))]);
    assert_eq!(inl("hi @everyone and @here!"), vec![t("hi "), Inline::Everyone, t(" and "), Inline::Here, t("!")]);
    assert_eq!(inl("mail@here.com"), vec![t("mail@here.com")]);
    assert_eq!(inl("@everyone2"), vec![t("@everyone2")]);
    assert_eq!(inl("<@abc>"), vec![t("<@abc>")]);
    assert_eq!(inl("<@123"), vec![t("<@123")]);
    assert_eq!(inl("<@99999999999999999999999>"), vec![t("<@99999999999999999999999>")]);
    assert_eq!(inl("**<@123>**"), vec![Inline::Bold(vec![Inline::UserMention(Id(123))])]);
    assert_eq!(
        inl("||hey <@1> <#2>||"),
        vec![Inline::Spoiler(vec![t("hey "), Inline::UserMention(Id(1)), t(" "), Inline::ChannelMention(Id(2))])]
    );
}

#[test]
fn custom_emoji_timestamps_and_commands() {
    assert_eq!(inl("<:blob:123>"), vec![Inline::CustomEmoji { name: "blob".into(), id: Id(123), animated: false }]);
    assert_eq!(inl("<a:dance_1:456>"), vec![Inline::CustomEmoji { name: "dance_1".into(), id: Id(456), animated: true }]);
    assert_eq!(inl("<:x:1>"), vec![t("<:x:1>")]);
    assert_eq!(inl("<:bad name:1>"), vec![t("<:bad name:1>")]);
    assert_eq!(inl("<t:1618953630>"), vec![Inline::Timestamp { unix: 1_618_953_630, style: 'f' }]);
    assert_eq!(inl("<t:1618953630:R>"), vec![Inline::Timestamp { unix: 1_618_953_630, style: 'R' }]);
    assert_eq!(inl("<t:-5:D>"), vec![Inline::Timestamp { unix: -5, style: 'D' }]);
    assert_eq!(inl("<t:1:Z>"), vec![t("<t:1:Z>")]);
    assert_eq!(inl("<t:abc>"), vec![t("<t:abc>")]);
    assert_eq!(inl("</ping:123>"), vec![Inline::SlashCommand { name: "ping".into(), id: Id(123) }]);
    assert_eq!(inl("</config set:9>"), vec![Inline::SlashCommand { name: "config set".into(), id: Id(9) }]);
    assert_eq!(inl("</a b c d:9>"), vec![t("</a b c d:9>")]);
    assert_eq!(inl("</:9>"), vec![t("</:9>")]);
}

#[test]
fn line_breaks() {
    assert_eq!(inl("a\nb"), vec![t("a"), Inline::LineBreak, t("b")]);
    assert_eq!(parse("a\n\nb"), vec![Block::Paragraph(vec![t("a"), Inline::LineBreak, Inline::LineBreak, t("b")])]);
    assert_eq!(parse("a\r\nb"), vec![Block::Paragraph(vec![t("a"), Inline::LineBreak, t("b")])]);
    assert_eq!(parse("**a\nb**"), vec![Block::Paragraph(vec![Inline::Bold(vec![t("a"), Inline::LineBreak, t("b")])])]);
}

#[test]
fn code_blocks() {
    assert_eq!(parse("```rust\nfn main() {}\n```"), vec![Block::CodeBlock { lang: Some("rust".into()), code: "fn main() {}".into() }]);
    assert_eq!(parse("```\nplain\n```"), vec![Block::CodeBlock { lang: None, code: "plain".into() }]);
    assert_eq!(parse("```inline```"), vec![Block::CodeBlock { lang: None, code: "inline".into() }]);
    assert_eq!(parse("```js code```"), vec![Block::CodeBlock { lang: None, code: "js code".into() }]);
    assert_eq!(
        parse("```py\n  indented\n\n  more\n```"),
        vec![Block::CodeBlock { lang: Some("py".into()), code: "  indented\n\n  more".into() }]
    );
    assert_eq!(parse("```**not bold** <@1>```"), vec![Block::CodeBlock { lang: None, code: "**not bold** <@1>".into() }]);
}

#[test]
fn unterminated_or_empty_fence_is_text() {
    assert_eq!(parse("```rust\nfn main"), vec![Block::Paragraph(vec![t("```rust"), Inline::LineBreak, t("fn main")])]);
    assert_eq!(parse("``````"), vec![Block::Paragraph(vec![t("``````")])]);
    assert_eq!(parse("a ``` b"), vec![Block::Paragraph(vec![t("a ``` b")])]);
}

#[test]
fn code_block_amid_text() {
    assert_eq!(
        parse("before\n```\ncode\n```\nafter"),
        vec![Block::Paragraph(vec![t("before")]), Block::CodeBlock { lang: None, code: "code".into() }, Block::Paragraph(vec![t("after")]),]
    );
    assert_eq!(
        parse("see ```x``` here"),
        vec![Block::Paragraph(vec![t("see")]), Block::CodeBlock { lang: None, code: "x".into() }, Block::Paragraph(vec![t("here")]),]
    );
    assert_eq!(
        parse("```a```\n```b```"),
        vec![Block::CodeBlock { lang: None, code: "a".into() }, Block::CodeBlock { lang: None, code: "b".into() }]
    );
}

#[test]
fn quotes() {
    assert_eq!(parse("> hi"), vec![Block::Quote(vec![Block::Paragraph(vec![t("hi")])])]);
    assert_eq!(
        parse("> a\n> b\nplain"),
        vec![Block::Quote(vec![Block::Paragraph(vec![t("a"), Inline::LineBreak, t("b")])]), Block::Paragraph(vec![t("plain")])]
    );
    assert_eq!(
        parse(">>> a\nb\n**c**"),
        vec![Block::Quote(vec![Block::Paragraph(vec![t("a"), Inline::LineBreak, t("b"), Inline::LineBreak, Inline::Bold(vec![t("c")])])])]
    );
    assert_eq!(parse("not > a quote"), para("not > a quote"));
    assert_eq!(parse(">no space"), para(">no space"));
    assert_eq!(parse(">>>nospace"), para(">>>nospace"));
    assert_eq!(parse("> a\n\n> b").len(), 2);
}

#[test]
fn quotes_hold_blocks_and_do_not_nest() {
    assert_eq!(
        parse("> # Title\n> text"),
        vec![Block::Quote(vec![Block::Heading { level: 1, content: vec![t("Title")] }, Block::Paragraph(vec![t("text")])])]
    );
    assert_eq!(
        parse("> ```rs\n> let x = 1;\n> ```"),
        vec![Block::Quote(vec![Block::CodeBlock { lang: Some("rs".into()), code: "let x = 1;".into() }])]
    );
    assert_eq!(parse("> > nested"), vec![Block::Quote(vec![Block::Paragraph(vec![t("> nested")])])]);
    match &parse("> - a\n> - b")[0] {
        Block::Quote(inner) => assert!(matches!(&inner[0], Block::List { ordered: false, items, .. } if items.len() == 2)),
        other => panic!("{other:?}"),
    }
}

#[test]
fn headings_and_subtext() {
    assert_eq!(parse("# Big"), vec![Block::Heading { level: 1, content: vec![t("Big")] }]);
    assert_eq!(parse("## Mid **x**"), vec![Block::Heading { level: 2, content: vec![t("Mid "), Inline::Bold(vec![t("x")])] }]);
    assert_eq!(parse("### Small"), vec![Block::Heading { level: 3, content: vec![t("Small")] }]);
    assert_eq!(parse("#### Four"), para("#### Four"));
    assert_eq!(parse("#nospace"), para("#nospace"));
    assert_eq!(parse("# "), para("#"));
    assert_eq!(parse("text # not heading"), para("text # not heading"));
    assert_eq!(parse("# H\nbody"), vec![Block::Heading { level: 1, content: vec![t("H")] }, Block::Paragraph(vec![t("body")])]);
    assert_eq!(parse("-# small print"), vec![Block::Subtext(vec![t("small print")])]);
    assert_eq!(parse("-#nospace"), para("-#nospace"));
}

#[test]
fn lists() {
    let item = |s: &str| vec![Block::Paragraph(vec![t(s)])];
    assert_eq!(parse("- a\n- b"), vec![Block::List { ordered: false, start: 0, items: vec![item("a"), item("b")] }]);
    assert_eq!(parse("* a\n* b"), vec![Block::List { ordered: false, start: 0, items: vec![item("a"), item("b")] }]);
    assert_eq!(parse("1. a\n2. b"), vec![Block::List { ordered: true, start: 1, items: vec![item("a"), item("b")] }]);
    assert_eq!(parse("3. x\n4. y"), vec![Block::List { ordered: true, start: 3, items: vec![item("x"), item("y")] }]);
    assert_eq!(
        parse("- a **b**"),
        vec![Block::List { ordered: false, start: 0, items: vec![vec![Block::Paragraph(vec![t("a "), Inline::Bold(vec![t("b")])])]] }]
    );
    assert_eq!(parse("-x"), para("-x"));
    assert_eq!(parse("1.x"), para("1.x"));
    assert_eq!(parse("- "), para("-"));
    assert_eq!(
        parse("- a\ntext"),
        vec![Block::List { ordered: false, start: 0, items: vec![item("a")] }, Block::Paragraph(vec![t("text")])]
    );
}

#[test]
fn nested_lists() {
    let item = |s: &str| Block::Paragraph(vec![t(s)]);
    assert_eq!(
        parse("- a\n  - b\n  - c\n- d"),
        vec![Block::List {
            ordered: false,
            start: 0,
            items: vec![
                vec![item("a"), Block::List { ordered: false, start: 0, items: vec![vec![item("b")], vec![item("c")]] }],
                vec![item("d")],
            ]
        }]
    );
    assert_eq!(
        parse("1. a\n   - b\n2. c"),
        vec![Block::List {
            ordered: true,
            start: 1,
            items: vec![vec![item("a"), Block::List { ordered: false, start: 0, items: vec![vec![item("b")]] }], vec![item("c")],]
        }]
    );
    // Switching list kind at the same level starts a new list.
    assert_eq!(parse("- a\n1. b").len(), 2);
}

#[test]
fn plain_text_rendering() {
    let resolve = |i: &Inline| match i {
        Inline::UserMention(Id(1)) => Some("@alice".to_string()),
        Inline::ChannelMention(Id(2)) => Some("#general".to_string()),
        _ => None,
    };
    let blocks = parse("hey <@1> in <#2> <@9> <@&3> @everyone <:blob:5> ||secret|| `code` **b** [l](https://x.y)");
    assert_eq!(to_plain_text(&blocks, &resolve), "hey @alice in #general @user @role @everyone :blob: [spoiler] code b l");
    assert_eq!(plain("# T\n> q\n```rs\nlet a;\n```\n-# s"), "T\nq\nlet a;\ns");
    assert_eq!(plain("- a\n  - b\n1. c"), "• a\n  • b\n1. c");
    assert_eq!(plain("a\nb"), "a\nb");
    assert_eq!(plain("<https://a.com>"), "https://a.com");
    assert_eq!(plain("</ping:1>"), "/ping");
    assert!(!plain("<t:1618935630>").is_empty());
}

#[test]
fn deep_nesting_is_capped() {
    fn depth(i: &[Inline]) -> usize {
        i.iter()
            .map(|n| match n {
                Inline::Bold(c) | Inline::Italic(c) | Inline::Underline(c) | Inline::Strike(c) | Inline::Spoiler(c) => 1 + depth(c),
                Inline::Link { text, .. } => 1 + depth(text),
                _ => 0,
            })
            .max()
            .unwrap_or(0)
    }
    let src = format!("{}x{}", "*_~~||".repeat(500), "||~~_*".repeat(500));
    assert!(depth(&inl(&src)) <= MAX_DEPTH);
    let src = format!("{}x{}", "**".repeat(5000), "**".repeat(5000));
    assert!(depth(&inl(&src)) <= MAX_DEPTH);
    let src = "- a\n".to_string() + &(1..200).map(|i| format!("{}- x\n", " ".repeat(2 * i))).collect::<String>();
    assert!(!parse(&src).is_empty());
}

#[test]
fn realistic_messages() {
    let blocks = parse("**Release 1.2** is out!\n> thanks <@42>\n- fast\n- stable\nDownload: https://example.com/dl_1_2.zip.");
    assert_eq!(blocks.len(), 4);
    assert!(matches!(&blocks[3], Block::Paragraph(p) if p.contains(&Inline::AutoLink("https://example.com/dl_1_2.zip".into()))));
}
