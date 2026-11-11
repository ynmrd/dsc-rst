//! Typed gateway dispatch events (opcode 0). Unknown event names are preserved as
//! `Dispatch::Unknown` so new Discord events never break the connection.

use serde::de::DeserializeOwned;

use crate::model::*;

#[derive(Debug, Clone, PartialEq)]
pub enum Dispatch {
    Ready(Box<Ready>),
    Resumed,
    GuildCreate(Box<Guild>),
    GuildUpdate(Box<Guild>),
    GuildDelete(UnavailableGuild),
    ChannelCreate(Box<Channel>),
    ChannelUpdate(Box<Channel>),
    ChannelDelete(Box<Channel>),
    ChannelPinsUpdate(ChannelPinsUpdate),
    ThreadCreate(Box<Channel>),
    ThreadUpdate(Box<Channel>),
    ThreadDelete(Box<Channel>),
    ThreadListSync(Box<ThreadListSync>),
    GuildRoleCreate(Box<GuildRoleEvent>),
    GuildRoleUpdate(Box<GuildRoleEvent>),
    GuildRoleDelete(GuildRoleDelete),
    GuildMemberAdd(Box<Member>),
    GuildMemberUpdate(Box<Member>),
    GuildMemberRemove(Box<GuildMemberRemove>),
    GuildMembersChunk(Box<GuildMembersChunk>),
    GuildEmojisUpdate(Box<GuildEmojisUpdate>),
    GuildStickersUpdate(Box<GuildStickersUpdate>),
    MessageCreate(Box<Message>),
    MessageUpdate(Box<MessageUpdate>),
    MessageDelete(MessageDelete),
    MessageDeleteBulk(Box<MessageDeleteBulk>),
    MessageReactionAdd(Box<ReactionEvent>),
    MessageReactionRemove(Box<ReactionEvent>),
    MessageReactionRemoveAll(ReactionRemoveAll),
    MessageReactionRemoveEmoji(ReactionRemoveAll),
    PresenceUpdate(Box<Presence>),
    TypingStart(Box<TypingStart>),
    UserUpdate(Box<User>),
    VoiceStateUpdate(Box<VoiceState>),
    VoiceServerUpdate(VoiceServerUpdate),
    Unknown(String),
}

#[derive(Debug, thiserror::Error)]
#[error("failed to parse {event}: {source}")]
pub struct DispatchError {
    pub event: String,
    #[source]
    pub source: serde_json::Error,
}

impl Dispatch {
    /// Parse the `d` payload of a dispatch whose `t` is `event`.
    pub fn parse(event: &str, data: &str) -> Result<Dispatch, DispatchError> {
        fn p<T: DeserializeOwned>(event: &str, data: &str) -> Result<T, DispatchError> {
            serde_json::from_str(data).map_err(|source| DispatchError { event: event.to_string(), source })
        }
        Ok(match event {
            "READY" => Dispatch::Ready(p(event, data)?),
            "RESUMED" => Dispatch::Resumed,
            "GUILD_CREATE" => Dispatch::GuildCreate(p(event, data)?),
            "GUILD_UPDATE" => Dispatch::GuildUpdate(p(event, data)?),
            "GUILD_DELETE" => Dispatch::GuildDelete(p(event, data)?),
            "CHANNEL_CREATE" => Dispatch::ChannelCreate(p(event, data)?),
            "CHANNEL_UPDATE" => Dispatch::ChannelUpdate(p(event, data)?),
            "CHANNEL_DELETE" => Dispatch::ChannelDelete(p(event, data)?),
            "CHANNEL_PINS_UPDATE" => Dispatch::ChannelPinsUpdate(p(event, data)?),
            "THREAD_CREATE" => Dispatch::ThreadCreate(p(event, data)?),
            "THREAD_UPDATE" => Dispatch::ThreadUpdate(p(event, data)?),
            "THREAD_DELETE" => Dispatch::ThreadDelete(p(event, data)?),
            "THREAD_LIST_SYNC" => Dispatch::ThreadListSync(p(event, data)?),
            "GUILD_ROLE_CREATE" => Dispatch::GuildRoleCreate(p(event, data)?),
            "GUILD_ROLE_UPDATE" => Dispatch::GuildRoleUpdate(p(event, data)?),
            "GUILD_ROLE_DELETE" => Dispatch::GuildRoleDelete(p(event, data)?),
            "GUILD_MEMBER_ADD" => Dispatch::GuildMemberAdd(p(event, data)?),
            "GUILD_MEMBER_UPDATE" => Dispatch::GuildMemberUpdate(p(event, data)?),
            "GUILD_MEMBER_REMOVE" => Dispatch::GuildMemberRemove(p(event, data)?),
            "GUILD_MEMBERS_CHUNK" => Dispatch::GuildMembersChunk(p(event, data)?),
            "GUILD_EMOJIS_UPDATE" => Dispatch::GuildEmojisUpdate(p(event, data)?),
            "GUILD_STICKERS_UPDATE" => Dispatch::GuildStickersUpdate(p(event, data)?),
            "MESSAGE_CREATE" => Dispatch::MessageCreate(p(event, data)?),
            "MESSAGE_UPDATE" => Dispatch::MessageUpdate(p(event, data)?),
            "MESSAGE_DELETE" => Dispatch::MessageDelete(p(event, data)?),
            "MESSAGE_DELETE_BULK" => Dispatch::MessageDeleteBulk(p(event, data)?),
            "MESSAGE_REACTION_ADD" => Dispatch::MessageReactionAdd(p(event, data)?),
            "MESSAGE_REACTION_REMOVE" => Dispatch::MessageReactionRemove(p(event, data)?),
            "MESSAGE_REACTION_REMOVE_ALL" => Dispatch::MessageReactionRemoveAll(p(event, data)?),
            "MESSAGE_REACTION_REMOVE_EMOJI" => Dispatch::MessageReactionRemoveEmoji(p(event, data)?),
            "PRESENCE_UPDATE" => Dispatch::PresenceUpdate(p(event, data)?),
            "TYPING_START" => Dispatch::TypingStart(p(event, data)?),
            "USER_UPDATE" => Dispatch::UserUpdate(p(event, data)?),
            "VOICE_STATE_UPDATE" => Dispatch::VoiceStateUpdate(p(event, data)?),
            "VOICE_SERVER_UPDATE" => Dispatch::VoiceServerUpdate(p(event, data)?),
            other => Dispatch::Unknown(other.to_string()),
        })
    }

    pub fn name(&self) -> &str {
        match self {
            Dispatch::Ready(_) => "READY",
            Dispatch::Resumed => "RESUMED",
            Dispatch::GuildCreate(_) => "GUILD_CREATE",
            Dispatch::GuildUpdate(_) => "GUILD_UPDATE",
            Dispatch::GuildDelete(_) => "GUILD_DELETE",
            Dispatch::ChannelCreate(_) => "CHANNEL_CREATE",
            Dispatch::ChannelUpdate(_) => "CHANNEL_UPDATE",
            Dispatch::ChannelDelete(_) => "CHANNEL_DELETE",
            Dispatch::ChannelPinsUpdate(_) => "CHANNEL_PINS_UPDATE",
            Dispatch::ThreadCreate(_) => "THREAD_CREATE",
            Dispatch::ThreadUpdate(_) => "THREAD_UPDATE",
            Dispatch::ThreadDelete(_) => "THREAD_DELETE",
            Dispatch::ThreadListSync(_) => "THREAD_LIST_SYNC",
            Dispatch::GuildRoleCreate(_) => "GUILD_ROLE_CREATE",
            Dispatch::GuildRoleUpdate(_) => "GUILD_ROLE_UPDATE",
            Dispatch::GuildRoleDelete(_) => "GUILD_ROLE_DELETE",
            Dispatch::GuildMemberAdd(_) => "GUILD_MEMBER_ADD",
            Dispatch::GuildMemberUpdate(_) => "GUILD_MEMBER_UPDATE",
            Dispatch::GuildMemberRemove(_) => "GUILD_MEMBER_REMOVE",
            Dispatch::GuildMembersChunk(_) => "GUILD_MEMBERS_CHUNK",
            Dispatch::GuildEmojisUpdate(_) => "GUILD_EMOJIS_UPDATE",
            Dispatch::GuildStickersUpdate(_) => "GUILD_STICKERS_UPDATE",
            Dispatch::MessageCreate(_) => "MESSAGE_CREATE",
            Dispatch::MessageUpdate(_) => "MESSAGE_UPDATE",
            Dispatch::MessageDelete(_) => "MESSAGE_DELETE",
            Dispatch::MessageDeleteBulk(_) => "MESSAGE_DELETE_BULK",
            Dispatch::MessageReactionAdd(_) => "MESSAGE_REACTION_ADD",
            Dispatch::MessageReactionRemove(_) => "MESSAGE_REACTION_REMOVE",
            Dispatch::MessageReactionRemoveAll(_) => "MESSAGE_REACTION_REMOVE_ALL",
            Dispatch::MessageReactionRemoveEmoji(_) => "MESSAGE_REACTION_REMOVE_EMOJI",
            Dispatch::PresenceUpdate(_) => "PRESENCE_UPDATE",
            Dispatch::TypingStart(_) => "TYPING_START",
            Dispatch::UserUpdate(_) => "USER_UPDATE",
            Dispatch::VoiceStateUpdate(_) => "VOICE_STATE_UPDATE",
            Dispatch::VoiceServerUpdate(_) => "VOICE_SERVER_UPDATE",
            Dispatch::Unknown(n) => n,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_known_and_unknown() {
        let d = Dispatch::parse("TYPING_START", r#"{"channel_id":"1","user_id":"2","timestamp":1700000000}"#).unwrap();
        assert!(matches!(d, Dispatch::TypingStart(ref t) if t.user_id.0 == 2));
        assert_eq!(d.name(), "TYPING_START");
        let d = Dispatch::parse("SOME_FUTURE_EVENT", "{}").unwrap();
        assert_eq!(d, Dispatch::Unknown("SOME_FUTURE_EVENT".into()));
    }

    #[test]
    fn reaction_remove_emoji_has_emoji() {
        let d = Dispatch::parse("MESSAGE_REACTION_REMOVE_EMOJI", r#"{"channel_id":"1","message_id":"2","emoji":{"id":null,"name":"🔥"}}"#)
            .unwrap();
        let Dispatch::MessageReactionRemoveEmoji(r) = d else { panic!() };
        assert_eq!(r.emoji.unwrap().name.as_deref(), Some("🔥"));
    }

    #[test]
    fn bad_payload_is_error_with_event_name() {
        let e = Dispatch::parse("MESSAGE_CREATE", r#"{"id":{}}"#).unwrap_err();
        assert_eq!(e.event, "MESSAGE_CREATE");
    }
}
