use dn_core::{Id, Message};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    All,
    Mentions,
    Nothing,
}

/// Everything about the receiving side that decides whether a message may notify.
pub struct NotifyContext<'a> {
    pub self_user_id: Id,
    pub self_role_ids: &'a [Id],
    /// Effective level for the message's channel (channel, then guild, then default).
    pub level: NotifyLevel,
    pub muted: bool,
    pub suppress_everyone: bool,
    /// DM or group DM.
    pub is_private: bool,
    /// The channel is open in a focused window, so the user is already looking at it.
    pub viewing_channel_focused: bool,
    pub dnd: bool,
}

/// Whether `msg` should raise a desktop notification.
///
/// Direct messages are addressed to the user by definition, so they notify under both
/// `All` and `Mentions`; `Nothing` silences them too.
pub fn should_notify(msg: &Message, ctx: &NotifyContext) -> bool {
    if msg.author.id == ctx.self_user_id || ctx.muted || ctx.dnd || ctx.viewing_channel_focused {
        return false;
    }
    match ctx.level {
        NotifyLevel::Nothing => false,
        NotifyLevel::All => true,
        NotifyLevel::Mentions => {
            ctx.is_private || is_mention(msg, ctx.self_user_id, ctx.self_role_ids, ctx.suppress_everyone)
        }
    }
}

/// Direct mention, mention of one of the user's roles, `@everyone`/`@here` (unless
/// suppressed), or a reply to one of the user's own messages.
pub fn is_mention(msg: &Message, self_user_id: Id, self_role_ids: &[Id], suppress_everyone: bool) -> bool {
    msg.mentions.iter().any(|u| u.id == self_user_id)
        || msg.mention_roles.iter().any(|r| self_role_ids.contains(r))
        || (msg.mention_everyone && !suppress_everyone)
        || (msg.is_reply() && msg.referenced_message.as_ref().is_some_and(|m| m.author.id == self_user_id))
}

#[cfg(test)]
mod tests {
    use dn_core::User;

    use super::*;

    const ME: Id = Id(100);
    const OTHER: Id = Id(200);
    const MY_ROLE: Id = Id(7);

    fn message(author: Id) -> Message {
        Message { id: Id(1), channel_id: Id(2), author: User { id: author, ..User::default() }, content: "hi".into(), ..Message::default() }
    }

    fn ctx(roles: &[Id]) -> NotifyContext<'_> {
        NotifyContext {
            self_user_id: ME,
            self_role_ids: roles,
            level: NotifyLevel::All,
            muted: false,
            suppress_everyone: false,
            is_private: false,
            viewing_channel_focused: false,
            dnd: false,
        }
    }

    fn mentioning_me() -> Message {
        let mut m = message(OTHER);
        m.mentions.push(User { id: ME, ..User::default() });
        m
    }

    #[test]
    fn plain_message_notifies_at_level_all() {
        assert!(should_notify(&message(OTHER), &ctx(&[])));
    }

    #[test]
    fn own_messages_never_notify() {
        let mut c = ctx(&[]);
        assert!(!should_notify(&message(ME), &c));
        c.is_private = true;
        assert!(!should_notify(&message(ME), &c));
    }

    #[test]
    fn muted_dnd_and_focused_channel_suppress_even_mentions() {
        let m = mentioning_me();
        let mut c = ctx(&[]);
        assert!(should_notify(&m, &c));
        c.muted = true;
        assert!(!should_notify(&m, &c));
        c.muted = false;
        c.dnd = true;
        assert!(!should_notify(&m, &c));
        c.dnd = false;
        c.viewing_channel_focused = true;
        assert!(!should_notify(&m, &c));
    }

    #[test]
    fn level_nothing_silences_everything_including_dms_and_mentions() {
        let mut c = ctx(&[]);
        c.level = NotifyLevel::Nothing;
        assert!(!should_notify(&mentioning_me(), &c));
        c.is_private = true;
        assert!(!should_notify(&message(OTHER), &c));
    }

    #[test]
    fn level_mentions_requires_a_mention_in_guild_channels() {
        let mut c = ctx(&[]);
        c.level = NotifyLevel::Mentions;
        assert!(!should_notify(&message(OTHER), &c));
        assert!(should_notify(&mentioning_me(), &c));

        let mut someone_else = message(OTHER);
        someone_else.mentions.push(User { id: Id(300), ..User::default() });
        assert!(!should_notify(&someone_else, &c));
    }

    #[test]
    fn direct_messages_notify_at_all_and_mentions() {
        let mut c = ctx(&[]);
        c.is_private = true;
        c.level = NotifyLevel::All;
        assert!(should_notify(&message(OTHER), &c));
        c.level = NotifyLevel::Mentions;
        assert!(should_notify(&message(OTHER), &c));
    }

    #[test]
    fn everyone_mentions_respect_suppression() {
        let mut m = message(OTHER);
        m.mention_everyone = true;
        let mut c = ctx(&[]);
        c.level = NotifyLevel::Mentions;
        assert!(should_notify(&m, &c));
        c.suppress_everyone = true;
        assert!(!should_notify(&m, &c));
        // A direct mention still counts when @everyone is suppressed.
        m.mentions.push(User { id: ME, ..User::default() });
        assert!(should_notify(&m, &c));
    }

    #[test]
    fn role_mentions_only_count_for_roles_the_user_has() {
        let mut m = message(OTHER);
        m.mention_roles.push(MY_ROLE);
        let roles = [Id(1), MY_ROLE];
        let mut c = ctx(&roles);
        c.level = NotifyLevel::Mentions;
        assert!(should_notify(&m, &c));

        c.self_role_ids = &[Id(1), Id(2)];
        assert!(!should_notify(&m, &c));
        c.self_role_ids = &[];
        assert!(!should_notify(&m, &c));
    }

    #[test]
    fn replies_to_own_messages_count_as_mentions() {
        let mut reply = message(OTHER);
        reply.kind = dn_core::message_kind::REPLY;
        reply.referenced_message = Some(Box::new(message(ME)));
        let mut c = ctx(&[]);
        c.level = NotifyLevel::Mentions;
        assert!(should_notify(&reply, &c));

        reply.referenced_message = Some(Box::new(message(Id(300))));
        assert!(!should_notify(&reply, &c));

        reply.referenced_message = None;
        assert!(!should_notify(&reply, &c));
    }

    #[test]
    fn is_mention_covers_each_source() {
        let roles = [MY_ROLE];
        assert!(!is_mention(&message(OTHER), ME, &roles, false));
        assert!(is_mention(&mentioning_me(), ME, &roles, true));

        let mut role = message(OTHER);
        role.mention_roles.push(MY_ROLE);
        assert!(is_mention(&role, ME, &roles, true));

        let mut everyone = message(OTHER);
        everyone.mention_everyone = true;
        assert!(is_mention(&everyone, ME, &roles, false));
        assert!(!is_mention(&everyone, ME, &roles, true));
    }
}
