//! Discord permission bit flags and the documented permission computation algorithm
//! (base permissions from roles, then channel overwrites in everyone → roles → member order).

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::id::Id;
use crate::model::{Guild, Member, Overwrite, OverwriteKind, Role};

#[derive(Copy, Clone, PartialEq, Eq, Hash, Default)]
pub struct Permissions(pub u64);

macro_rules! perms {
    ($($name:ident = $bit:expr;)*) => {
        impl Permissions {
            $(pub const $name: Permissions = Permissions(1 << $bit);)*
            pub const ALL_KNOWN: Permissions = Permissions(0 $(| (1 << $bit))*);
        }
    };
}

perms! {
    CREATE_INSTANT_INVITE = 0;
    KICK_MEMBERS = 1;
    BAN_MEMBERS = 2;
    ADMINISTRATOR = 3;
    MANAGE_CHANNELS = 4;
    MANAGE_GUILD = 5;
    ADD_REACTIONS = 6;
    VIEW_AUDIT_LOG = 7;
    PRIORITY_SPEAKER = 8;
    STREAM = 9;
    VIEW_CHANNEL = 10;
    SEND_MESSAGES = 11;
    SEND_TTS_MESSAGES = 12;
    MANAGE_MESSAGES = 13;
    EMBED_LINKS = 14;
    ATTACH_FILES = 15;
    READ_MESSAGE_HISTORY = 16;
    MENTION_EVERYONE = 17;
    USE_EXTERNAL_EMOJIS = 18;
    VIEW_GUILD_INSIGHTS = 19;
    CONNECT = 20;
    SPEAK = 21;
    MUTE_MEMBERS = 22;
    DEAFEN_MEMBERS = 23;
    MOVE_MEMBERS = 24;
    USE_VAD = 25;
    CHANGE_NICKNAME = 26;
    MANAGE_NICKNAMES = 27;
    MANAGE_ROLES = 28;
    MANAGE_WEBHOOKS = 29;
    MANAGE_GUILD_EXPRESSIONS = 30;
    USE_APPLICATION_COMMANDS = 31;
    REQUEST_TO_SPEAK = 32;
    MANAGE_EVENTS = 33;
    MANAGE_THREADS = 34;
    CREATE_PUBLIC_THREADS = 35;
    CREATE_PRIVATE_THREADS = 36;
    USE_EXTERNAL_STICKERS = 37;
    SEND_MESSAGES_IN_THREADS = 38;
    USE_EMBEDDED_ACTIVITIES = 39;
    MODERATE_MEMBERS = 40;
    VIEW_CREATOR_MONETIZATION_ANALYTICS = 41;
    USE_SOUNDBOARD = 42;
    CREATE_GUILD_EXPRESSIONS = 43;
    CREATE_EVENTS = 44;
    USE_EXTERNAL_SOUNDS = 45;
    SEND_VOICE_MESSAGES = 46;
    SEND_POLLS = 49;
    USE_EXTERNAL_APPS = 50;
    PIN_MESSAGES = 51;
}

impl Permissions {
    pub const NONE: Permissions = Permissions(0);
    pub const ALL: Permissions = Permissions(u64::MAX);

    pub const fn contains(self, other: Permissions) -> bool {
        self.0 & other.0 == other.0
    }
    pub const fn union(self, other: Permissions) -> Permissions {
        Permissions(self.0 | other.0)
    }
    pub const fn remove(self, other: Permissions) -> Permissions {
        Permissions(self.0 & !other.0)
    }
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn can_view(self) -> bool {
        self.contains(Self::VIEW_CHANNEL)
    }
    pub fn can_send(self) -> bool {
        self.contains(Self::VIEW_CHANNEL.union(Self::SEND_MESSAGES))
    }
    pub fn can_read_history(self) -> bool {
        self.contains(Self::VIEW_CHANNEL.union(Self::READ_MESSAGE_HISTORY))
    }
    /// Pinning used to be covered by MANAGE_MESSAGES; Discord split it into PIN_MESSAGES.
    pub fn can_pin(self) -> bool {
        self.contains(Self::PIN_MESSAGES) || self.contains(Self::MANAGE_MESSAGES)
    }
}

impl std::ops::BitOr for Permissions {
    type Output = Permissions;
    fn bitor(self, rhs: Self) -> Self {
        self.union(rhs)
    }
}

impl fmt::Debug for Permissions {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Permissions({:#x})", self.0)
    }
}

impl Serialize for Permissions {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for Permissions {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Discord sends permissions as a decimal string; accept numbers too.
        let id = Id::deserialize(d)?;
        Ok(Permissions(id.0))
    }
}

/// Guild-level ("base") permissions of a member, per the Discord docs.
pub fn base_permissions(guild: &Guild, member_user_id: Id, member_roles: &[Id]) -> Permissions {
    if guild.owner_id == Some(member_user_id) {
        return Permissions::ALL;
    }
    let everyone = guild.roles.iter().find(|r| r.id == guild.id);
    let mut perms = everyone.map(|r| r.permissions).unwrap_or_default();
    for role in roles_of(&guild.roles, member_roles) {
        perms = perms | role.permissions;
    }
    if perms.contains(Permissions::ADMINISTRATOR) {
        return Permissions::ALL;
    }
    perms
}

fn roles_of<'a>(roles: &'a [Role], ids: &'a [Id]) -> impl Iterator<Item = &'a Role> + 'a {
    roles.iter().filter(move |r| ids.contains(&r.id))
}

/// Apply channel permission overwrites on top of base permissions.
pub fn apply_overwrites(
    base: Permissions,
    guild_id: Id,
    member_user_id: Id,
    member_roles: &[Id],
    overwrites: &[Overwrite],
) -> Permissions {
    if base.contains(Permissions::ADMINISTRATOR) || base == Permissions::ALL {
        return Permissions::ALL;
    }
    let mut perms = base;
    if let Some(ow) = overwrites.iter().find(|o| o.kind == OverwriteKind::Role && o.id == guild_id) {
        perms = Permissions((perms.0 & !ow.deny.0) | ow.allow.0);
    }
    let (mut allow, mut deny) = (0u64, 0u64);
    for ow in overwrites
        .iter()
        .filter(|o| o.kind == OverwriteKind::Role && o.id != guild_id && member_roles.contains(&o.id))
    {
        allow |= ow.allow.0;
        deny |= ow.deny.0;
    }
    perms = Permissions((perms.0 & !deny) | allow);
    if let Some(ow) = overwrites.iter().find(|o| o.kind == OverwriteKind::Member && o.id == member_user_id) {
        perms = Permissions((perms.0 & !ow.deny.0) | ow.allow.0);
    }
    perms
}

/// Effective permissions of `member` in a channel with `overwrites`.
/// Implicit rules: without VIEW_CHANNEL nothing else applies; without SEND_MESSAGES,
/// mention/attach/embed permissions are irrelevant (callers check `can_send`).
pub fn channel_permissions(guild: &Guild, member_user_id: Id, member: &Member, overwrites: &[Overwrite]) -> Permissions {
    let base = base_permissions(guild, member_user_id, &member.roles);
    let p = apply_overwrites(base, guild.id, member_user_id, &member.roles, overwrites);
    if !p.contains(Permissions::VIEW_CHANNEL) {
        return Permissions::NONE;
    }
    p
}

#[cfg(test)]
mod tests {
    use super::*;

    fn role(id: u64, perms: Permissions) -> Role {
        Role { id: Id(id), permissions: perms, ..Default::default() }
    }

    fn guild() -> Guild {
        Guild {
            id: Id(1),
            owner_id: Some(Id(99)),
            roles: vec![
                role(1, Permissions::VIEW_CHANNEL | Permissions::SEND_MESSAGES | Permissions::READ_MESSAGE_HISTORY),
                role(10, Permissions::MANAGE_MESSAGES),
                role(11, Permissions::ADMINISTRATOR),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn owner_has_everything() {
        assert_eq!(base_permissions(&guild(), Id(99), &[]), Permissions::ALL);
    }

    #[test]
    fn roles_are_unioned_and_admin_short_circuits() {
        let g = guild();
        let p = base_permissions(&g, Id(5), &[Id(10)]);
        assert!(p.contains(Permissions::MANAGE_MESSAGES));
        assert!(p.can_send());
        assert_eq!(base_permissions(&g, Id(5), &[Id(11)]), Permissions::ALL);
    }

    #[test]
    fn overwrite_order_everyone_roles_member() {
        let g = guild();
        let ows = vec![
            Overwrite { id: Id(1), kind: OverwriteKind::Role, allow: Permissions::NONE, deny: Permissions::VIEW_CHANNEL },
            Overwrite { id: Id(10), kind: OverwriteKind::Role, allow: Permissions::VIEW_CHANNEL, deny: Permissions::NONE },
            Overwrite { id: Id(5), kind: OverwriteKind::Member, allow: Permissions::NONE, deny: Permissions::SEND_MESSAGES },
        ];
        let base = base_permissions(&g, Id(5), &[Id(10)]);
        let p = apply_overwrites(base, g.id, Id(5), &[Id(10)], &ows);
        assert!(p.can_view());
        assert!(!p.contains(Permissions::SEND_MESSAGES));
        // A member without role 10 cannot see the channel at all.
        let m = Member { roles: vec![], ..Default::default() };
        assert_eq!(channel_permissions(&g, Id(6), &m, &ows), Permissions::NONE);
    }

    #[test]
    fn permissions_parse_from_string() {
        let p: Permissions = serde_json::from_str("\"4503599627370495\"").unwrap();
        assert!(p.contains(Permissions::PIN_MESSAGES));
        assert!(p.can_pin());
    }
}
