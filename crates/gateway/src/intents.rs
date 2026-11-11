use std::ops::{BitAnd, BitOr, BitOrAssign, Not};

/// Gateway intents bitfield (API v10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct Intents(pub u64);

impl Intents {
    pub const GUILDS: Intents = Intents(1 << 0);
    /// Privileged.
    pub const GUILD_MEMBERS: Intents = Intents(1 << 1);
    pub const GUILD_MODERATION: Intents = Intents(1 << 2);
    pub const GUILD_EXPRESSIONS: Intents = Intents(1 << 3);
    pub const GUILD_INTEGRATIONS: Intents = Intents(1 << 4);
    pub const GUILD_WEBHOOKS: Intents = Intents(1 << 5);
    pub const GUILD_INVITES: Intents = Intents(1 << 6);
    pub const GUILD_VOICE_STATES: Intents = Intents(1 << 7);
    /// Privileged.
    pub const GUILD_PRESENCES: Intents = Intents(1 << 8);
    pub const GUILD_MESSAGES: Intents = Intents(1 << 9);
    pub const GUILD_MESSAGE_REACTIONS: Intents = Intents(1 << 10);
    pub const GUILD_MESSAGE_TYPING: Intents = Intents(1 << 11);
    pub const DIRECT_MESSAGES: Intents = Intents(1 << 12);
    pub const DIRECT_MESSAGE_REACTIONS: Intents = Intents(1 << 13);
    pub const DIRECT_MESSAGE_TYPING: Intents = Intents(1 << 14);
    /// Privileged.
    pub const MESSAGE_CONTENT: Intents = Intents(1 << 15);
    pub const GUILD_SCHEDULED_EVENTS: Intents = Intents(1 << 16);
    pub const AUTO_MODERATION_CONFIGURATION: Intents = Intents(1 << 20);
    pub const AUTO_MODERATION_EXECUTION: Intents = Intents(1 << 21);
    pub const GUILD_MESSAGE_POLLS: Intents = Intents(1 << 24);
    pub const DIRECT_MESSAGE_POLLS: Intents = Intents(1 << 25);

    /// Intents that must be enabled for the application in the Developer Portal.
    pub const PRIVILEGED: Intents = Intents(Self::GUILD_MEMBERS.0 | Self::GUILD_PRESENCES.0 | Self::MESSAGE_CONTENT.0);

    pub const fn empty() -> Self {
        Intents(0)
    }

    /// Everything a chat client needs, including the three privileged intents.
    pub const fn default_client() -> Self {
        Intents(
            Self::GUILDS.0
                | Self::GUILD_MEMBERS.0
                | Self::GUILD_EXPRESSIONS.0
                | Self::GUILD_VOICE_STATES.0
                | Self::GUILD_PRESENCES.0
                | Self::GUILD_MESSAGES.0
                | Self::GUILD_MESSAGE_REACTIONS.0
                | Self::GUILD_MESSAGE_TYPING.0
                | Self::DIRECT_MESSAGES.0
                | Self::DIRECT_MESSAGE_REACTIONS.0
                | Self::DIRECT_MESSAGE_TYPING.0
                | Self::MESSAGE_CONTENT.0
                | Self::GUILD_MESSAGE_POLLS.0
                | Self::DIRECT_MESSAGE_POLLS.0,
        )
    }

    /// Same set minus privileged intents (fallback after close code 4014).
    pub const fn without_privileged(self) -> Self {
        Intents(self.0 & !Self::PRIVILEGED.0)
    }

    pub const fn contains(self, other: Intents) -> bool {
        self.0 & other.0 == other.0
    }

    pub const fn bits(self) -> u64 {
        self.0
    }
}

impl BitOr for Intents {
    type Output = Intents;
    fn bitor(self, rhs: Intents) -> Intents {
        Intents(self.0 | rhs.0)
    }
}

impl BitOrAssign for Intents {
    fn bitor_assign(&mut self, rhs: Intents) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for Intents {
    type Output = Intents;
    fn bitand(self, rhs: Intents) -> Intents {
        Intents(self.0 & rhs.0)
    }
}

impl Not for Intents {
    type Output = Intents;
    fn not(self) -> Intents {
        Intents(!self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn privileged_handling() {
        let d = Intents::default_client();
        assert!(d.contains(Intents::PRIVILEGED));
        assert!(d.contains(Intents::GUILDS | Intents::DIRECT_MESSAGES));
        let n = d.without_privileged();
        assert!(!n.contains(Intents::MESSAGE_CONTENT));
        assert!(n.contains(Intents::GUILD_MESSAGES));
        assert_eq!(Intents::PRIVILEGED.0, (1 << 1) | (1 << 8) | (1 << 15));
    }
}
