//! Discord-flavoured Markdown to AST.
//!
//! Block structure is parsed line by line; inline markup uses a delimiter-stack
//! algorithm so parsing stays linear on adversarial input. Nesting is capped at
//! [`MAX_DEPTH`]; deeper markers degrade to their contents.

use dn_core::Id;

mod block;
mod inline;
mod plain;
#[cfg(test)]
mod tests;

pub(crate) use inline::custom_emoji_at;

/// Maximum nesting of inline containers (and masked-link text recursion).
pub const MAX_DEPTH: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Inline {
    Text(String),
    Bold(Vec<Inline>),
    Italic(Vec<Inline>),
    Underline(Vec<Inline>),
    Strike(Vec<Inline>),
    Spoiler(Vec<Inline>),
    Code(String),
    Link {
        text: Vec<Inline>,
        url: String,
    },
    /// Bare `https://…` URL, or the `<https://…>` form (which suppresses embeds).
    AutoLink(String),
    UserMention(Id),
    RoleMention(Id),
    ChannelMention(Id),
    Everyone,
    Here,
    CustomEmoji {
        name: String,
        id: Id,
        animated: bool,
    },
    /// `<t:unix[:style]>`; `style` defaults to `'f'`.
    Timestamp {
        unix: i64,
        style: char,
    },
    /// `</name sub:id>`; `name` includes sub-command words separated by spaces.
    SlashCommand {
        name: String,
        id: Id,
    },
    LineBreak,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    CodeBlock { lang: Option<String>, code: String },
    Quote(Vec<Block>),
    Heading { level: u8, content: Vec<Inline> },
    List { ordered: bool, start: u32, items: Vec<Vec<Block>> },
    Subtext(Vec<Inline>),
}

/// Parses a full message body into blocks.
pub fn parse(src: &str) -> Vec<Block> {
    block::parse(src)
}

/// Parses inline markup only (embed titles, field values, …). Newlines become [`Inline::LineBreak`].
pub fn parse_inline(src: &str) -> Vec<Inline> {
    inline::parse_inline(src)
}

/// Flattens blocks to readable plain text.
///
/// `resolve` is offered every mention-like inline (user/role/channel mentions, `@everyone`,
/// `@here`, custom emoji, timestamps, slash commands); returning `None` falls back to a
/// generic rendering such as `@user` or `:name:`. Spoilers become `[spoiler]`; code text is kept.
pub fn to_plain_text(blocks: &[Block], resolve: &dyn Fn(&Inline) -> Option<String>) -> String {
    plain::to_plain_text(blocks, resolve)
}
