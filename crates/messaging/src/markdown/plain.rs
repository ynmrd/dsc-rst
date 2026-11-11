use super::{Block, Inline};

type Resolve<'a> = &'a dyn Fn(&Inline) -> Option<String>;

pub fn to_plain_text(blocks: &[Block], resolve: Resolve) -> String {
    let mut out = String::new();
    write_blocks(blocks, resolve, &mut out);
    out
}

fn newline(out: &mut String) {
    if !out.is_empty() && !out.ends_with('\n') {
        out.push('\n');
    }
}

fn write_blocks(blocks: &[Block], resolve: Resolve, out: &mut String) {
    for block in blocks {
        newline(out);
        match block {
            Block::Paragraph(i) | Block::Subtext(i) | Block::Heading { content: i, .. } => write_inlines(i, resolve, out),
            Block::CodeBlock { code, .. } => out.push_str(code),
            Block::Quote(children) => write_blocks(children, resolve, out),
            Block::List { ordered, start, items } => write_list(*ordered, *start, items, 0, resolve, out),
        }
    }
}

fn write_list(ordered: bool, start: u32, items: &[Vec<Block>], indent: usize, resolve: Resolve, out: &mut String) {
    for (k, item) in items.iter().enumerate() {
        newline(out);
        out.extend(std::iter::repeat_n(' ', indent));
        if ordered {
            out.push_str(&(u64::from(start) + k as u64).to_string());
            out.push_str(". ");
        } else {
            out.push_str("• ");
        }
        for block in item {
            match block {
                Block::Paragraph(i) => write_inlines(i, resolve, out),
                Block::List { ordered, start, items } => write_list(*ordered, *start, items, indent + 2, resolve, out),
                other => write_blocks(std::slice::from_ref(other), resolve, out),
            }
        }
    }
}

fn write_inlines(inlines: &[Inline], resolve: Resolve, out: &mut String) {
    for inline in inlines {
        match inline {
            Inline::Text(t) | Inline::Code(t) | Inline::AutoLink(t) => out.push_str(t),
            Inline::Bold(c) | Inline::Italic(c) | Inline::Underline(c) | Inline::Strike(c) => write_inlines(c, resolve, out),
            Inline::Link { text, .. } => write_inlines(text, resolve, out),
            Inline::Spoiler(_) => out.push_str("[spoiler]"),
            Inline::LineBreak => out.push('\n'),
            other => match resolve(other) {
                Some(s) => out.push_str(&s),
                None => out.push_str(&fallback(other)),
            },
        }
    }
}

fn fallback(i: &Inline) -> String {
    match i {
        Inline::UserMention(_) => "@user".into(),
        Inline::RoleMention(_) => "@role".into(),
        Inline::ChannelMention(_) => "#channel".into(),
        Inline::Everyone => "@everyone".into(),
        Inline::Here => "@here".into(),
        Inline::CustomEmoji { name, .. } => format!(":{name}:"),
        Inline::SlashCommand { name, .. } => format!("/{name}"),
        Inline::Timestamp { unix, style } => {
            let style = if *style == 'R' { 'f' } else { *style };
            crate::timestamp::format(*unix, style, *unix, true)
        }
        _ => String::new(),
    }
}
