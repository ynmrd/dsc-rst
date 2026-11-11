use dn_core::{Id, Message};

/// Notification bodies longer than this many characters are cut with an ellipsis.
const MAX_BODY_CHARS: usize = 200;
const MAX_TITLE_CHARS: usize = 128;
/// Longest token between `<` and `>` that is still considered a Discord mention/emoji.
const MAX_TOKEN_LEN: usize = 64;

#[derive(Debug, Clone)]
pub struct Notification {
    pub title: String,
    pub body: String,
    pub channel_id: Id,
    pub guild_id: Option<Id>,
    pub message_id: Id,
}

/// `author` is the display name to show, `location` something like `#general, My Server`
/// (empty for DMs). With `show_preview` off the content is never put in the notification.
pub fn build_notification(msg: &Message, author: &str, location: &str, show_preview: bool) -> Notification {
    let title = if location.is_empty() { author.to_owned() } else { format!("{author} ({location})") };
    let body = if show_preview { preview(msg) } else { "New message".to_owned() };
    Notification {
        title: truncate_chars(&title, MAX_TITLE_CHARS),
        body,
        channel_id: msg.channel_id,
        guild_id: msg.guild_id,
        message_id: msg.id,
    }
}

fn preview(msg: &Message) -> String {
    let text = collapse_whitespace(&render_mentions(&msg.content, msg));
    if !text.is_empty() {
        return truncate_chars(&text, MAX_BODY_CHARS);
    }
    match msg.attachments.len() {
        0 if !msg.sticker_items.is_empty() => "Sent a sticker".to_owned(),
        0 => "New message".to_owned(),
        1 => "Sent an attachment".to_owned(),
        n => format!("Sent {n} attachments"),
    }
}

/// Cuts to `max` characters (never inside a UTF-8 sequence), marking the cut with `…`.
fn truncate_chars(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_owned(),
        Some((idx, _)) => format!("{}…", s[..idx].trim_end()),
    }
}

fn collapse_whitespace(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Replaces raw Discord markup (`<@123>`, `<@&5>`, `<#9>`, `<:name:1>`, `</cmd:2>`) with
/// readable text. Unrecognised `<...>` sequences are left untouched.
fn render_mentions(content: &str, msg: &Message) -> String {
    let mut out = String::with_capacity(content.len());
    let mut rest = content;
    while let Some(start) = rest.find('<') {
        out.push_str(&rest[..start]);
        rest = &rest[start..];
        let replaced = rest[1..]
            .char_indices()
            .take(MAX_TOKEN_LEN)
            .find(|&(_, c)| c == '>')
            .and_then(|(end, _)| render_token(&rest[1..1 + end], msg).map(|text| (text, end + 2)));
        match replaced {
            Some((text, consumed)) => {
                out.push_str(&text);
                rest = &rest[consumed..];
            }
            None => {
                out.push('<');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn render_token(token: &str, msg: &Message) -> Option<String> {
    if let Some(id) = token.strip_prefix("@&") {
        return id.parse::<u64>().ok().map(|_| "@role".to_owned());
    }
    if let Some(id) = token.strip_prefix('@') {
        let id: Id = id.strip_prefix('!').unwrap_or(id).parse().ok()?;
        return Some(format!("@{}", mentioned_name(msg, id)));
    }
    if let Some(id) = token.strip_prefix('#') {
        return id.parse::<u64>().ok().map(|_| "#channel".to_owned());
    }
    if let Some(command) = token.strip_prefix('/') {
        let (name, id) = command.rsplit_once(':')?;
        return (id.parse::<u64>().is_ok() && !name.is_empty()).then(|| format!("/{name}"));
    }
    let emoji = token.strip_prefix("a:").or_else(|| token.strip_prefix(':'))?;
    let (name, id) = emoji.rsplit_once(':')?;
    (id.parse::<u64>().is_ok() && !name.is_empty()).then(|| format!(":{name}:"))
}

fn mentioned_name(msg: &Message, id: Id) -> String {
    msg.mentions
        .iter()
        .find(|u| u.id == id)
        .map(|u| u.member.as_ref().and_then(|m| m.nick.as_deref()).filter(|n| !n.is_empty()).unwrap_or_else(|| u.display_name()).to_owned())
        .unwrap_or_else(|| "user".to_owned())
}

#[cfg(test)]
mod tests {
    use dn_core::{Attachment, Member, StickerItem, User};

    use super::*;

    fn message(content: &str) -> Message {
        Message { id: Id(11), channel_id: Id(22), guild_id: Some(Id(33)), content: content.into(), ..Message::default() }
    }

    fn body(msg: &Message) -> String {
        build_notification(msg, "Alice", "", true).body
    }

    #[test]
    fn carries_identifiers_and_formats_title() {
        let n = build_notification(&message("hello"), "Alice", "#general, My Server", true);
        assert_eq!(n.title, "Alice (#general, My Server)");
        assert_eq!(n.body, "hello");
        assert_eq!((n.channel_id, n.guild_id, n.message_id), (Id(22), Some(Id(33)), Id(11)));

        let dm = build_notification(&message("hello"), "Alice", "", true);
        assert_eq!(dm.title, "Alice");
    }

    #[test]
    fn long_bodies_are_cut_on_a_char_boundary() {
        let text = "é".repeat(300);
        let b = body(&message(&text));
        assert_eq!(b.chars().count(), MAX_BODY_CHARS + 1);
        assert!(b.ends_with('…'));
        assert!(b.starts_with("éé"));

        let emoji = "😀".repeat(MAX_BODY_CHARS + 5);
        assert_eq!(body(&message(&emoji)).chars().count(), MAX_BODY_CHARS + 1);

        // Exactly at the limit is left alone.
        let exact = "ж".repeat(MAX_BODY_CHARS);
        assert_eq!(body(&message(&exact)), exact);
    }

    #[test]
    fn truncation_does_not_leave_a_dangling_space_before_the_ellipsis() {
        let text = format!("{} tail", "a".repeat(MAX_BODY_CHARS - 1));
        let b = body(&message(&text));
        assert_eq!(b, format!("{}…", "a".repeat(MAX_BODY_CHARS - 1)));
    }

    #[test]
    fn attachment_only_messages_describe_the_attachment() {
        let mut m = message("");
        m.attachments.push(Attachment::default());
        assert_eq!(body(&m), "Sent an attachment");
        m.attachments.push(Attachment::default());
        assert_eq!(body(&m), "Sent 2 attachments");

        let mut whitespace = message("  \n ");
        whitespace.attachments.push(Attachment::default());
        assert_eq!(body(&whitespace), "Sent an attachment");
    }

    #[test]
    fn text_wins_over_attachments() {
        let mut m = message("look at this");
        m.attachments.push(Attachment::default());
        assert_eq!(body(&m), "look at this");
    }

    #[test]
    fn stickers_and_empty_messages_have_fallback_text() {
        let mut m = message("");
        assert_eq!(body(&m), "New message");
        m.sticker_items.push(StickerItem::default());
        assert_eq!(body(&m), "Sent a sticker");
    }

    #[test]
    fn preview_off_never_leaks_content() {
        let mut m = message("secret");
        m.attachments.push(Attachment::default());
        let n = build_notification(&m, "Alice", "#general", false);
        assert_eq!(n.body, "New message");
        assert!(!n.title.contains("secret"));
    }

    #[test]
    fn newlines_are_collapsed() {
        assert_eq!(body(&message("one\n\ntwo   three")), "one two three");
    }

    #[test]
    fn mentions_and_emoji_are_rendered_readably() {
        let mut m = message("hey <@42> and <@!43>, ping <@&9> in <#77> <:pog:123> <a:dance:456> </ban:5> <@999>");
        m.mentions.push(User { id: Id(42), username: "bob".into(), global_name: Some("Bob".into()), ..User::default() });
        m.mentions.push(User {
            id: Id(43),
            username: "carol".into(),
            member: Some(Box::new(Member { nick: Some("Caz".into()), ..Member::default() })),
            ..User::default()
        });
        assert_eq!(body(&m), "hey @Bob and @Caz, ping @role in #channel :pog: :dance: /ban @user");
    }

    #[test]
    fn unrelated_angle_brackets_survive() {
        assert_eq!(body(&message("1 < 2 > 0, <b>bold</b>, <@abc>, <")), "1 < 2 > 0, <b>bold</b>, <@abc>, <");
        assert_eq!(body(&message("vec<é> и <мир>")), "vec<é> и <мир>");
    }

    #[test]
    fn long_titles_are_truncated() {
        let n = build_notification(&message("x"), &"n".repeat(300), "", true);
        assert_eq!(n.title.chars().count(), MAX_TITLE_CHARS + 1);
    }
}
