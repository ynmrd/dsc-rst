//! Derived views for the sidebar and member list.

use dn_core::permissions;
use dn_core::{Channel, ChannelKind, Id, Status};

use crate::Store;

#[derive(Debug)]
pub struct ChannelGroup<'a> {
    pub category: Option<&'a Channel>,
    pub channels: Vec<&'a Channel>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberRow {
    pub user_id: Id,
    pub name: String,
    pub color: Option<u32>,
    pub status: Status,
    pub activity: Option<String>,
    pub bot: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MemberGroup {
    pub title: String,
    pub members: Vec<MemberRow>,
}

pub const MAX_MEMBER_ROWS: usize = 2000;

/// Text-like channels sort above voice channels inside a category, like the official client.
fn kind_bucket(k: ChannelKind) -> u8 {
    if k.is_voice() { 1 } else { 0 }
}

impl Store {
    /// Visible channels of a guild grouped by category, in sidebar order.
    pub fn channel_tree(&self, guild: Id) -> Vec<ChannelGroup<'_>> {
        let Some(gs) = self.guilds.get(&guild) else { return vec![] };
        let mut cats: Vec<&Channel> = vec![];
        let mut others: Vec<&Channel> = vec![];
        for cid in &gs.channel_ids {
            let Some(c) = self.channels.get(cid) else { continue };
            match c.kind {
                ChannelKind::GuildCategory => cats.push(c),
                k if k.is_thread() => {}
                _ => {
                    if self.permissions_in(c.id).can_view() {
                        others.push(c);
                    }
                }
            }
        }
        cats.sort_by_key(|c| (c.position, c.id));
        others.sort_by_key(|c| (kind_bucket(c.kind), c.position, c.id));
        let mut groups = vec![ChannelGroup { category: None, channels: vec![] }];
        let mut index = std::collections::HashMap::new();
        for c in &cats {
            index.insert(c.id, groups.len());
            groups.push(ChannelGroup { category: Some(*c), channels: vec![] });
        }
        for c in others {
            let slot = c.parent_id.and_then(|p| index.get(&p).copied()).unwrap_or(0);
            groups[slot].channels.push(c);
        }
        groups.retain(|g| !g.channels.is_empty());
        groups
    }

    /// Active (non-archived) threads under a channel, most recently active first.
    pub fn threads_of(&self, parent: Id) -> Vec<&Channel> {
        let Some(gid) = self.guild_of(parent) else { return vec![] };
        let Some(gs) = self.guilds.get(&gid) else { return vec![] };
        let mut v: Vec<&Channel> =
            gs.thread_ids.iter().filter_map(|t| self.channels.get(t)).filter(|t| t.parent_id == Some(parent)).collect();
        v.sort_by_key(|t| std::cmp::Reverse(t.last_message_id.max(Some(t.id))));
        v
    }

    /// Member list grouped by hoisted role (online), then "Online", then "Offline".
    /// When `channel` is set, only members who can view it are listed.
    pub fn member_list(&self, guild: Id, channel: Option<Id>) -> Vec<MemberGroup> {
        let Some(gs) = self.guilds.get(&guild) else { return vec![] };
        let overwrites = channel.and_then(|c| self.channels.get(&c)).map(|c| {
            if c.kind.is_thread() {
                c.parent_id.and_then(|p| self.channels.get(&p)).map(|p| p.permission_overwrites.clone()).unwrap_or_default()
            } else {
                c.permission_overwrites.clone()
            }
        });
        let mut hoisted: Vec<&dn_core::Role> = gs.guild.roles.iter().filter(|r| r.hoist).collect();
        hoisted.sort_by(|a, b| b.position.cmp(&a.position).then(a.id.cmp(&b.id)));
        let mut buckets: Vec<Vec<MemberRow>> = vec![Vec::new(); hoisted.len() + 2];
        let online_idx = hoisted.len();
        let offline_idx = hoisted.len() + 1;
        let mut count = 0;
        for (uid, m) in &gs.members {
            if count >= MAX_MEMBER_ROWS {
                break;
            }
            if let Some(ow) = &overwrites
                && !permissions::channel_permissions(&gs.guild, *uid, m, ow).can_view()
            {
                continue;
            }
            let user = self.users.get(uid);
            let status = self.status_of(*uid);
            let presence = self.presences.get(uid);
            let row = MemberRow {
                user_id: *uid,
                name: self.display_name(Some(guild), *uid),
                color: self.member_color(Some(guild), *uid),
                status,
                activity: presence.and_then(|p| p.activities.first()).map(|a| a.describe()).filter(|s| !s.is_empty()),
                bot: user.is_some_and(|u| u.bot),
            };
            let slot = if status == Status::Offline || status == Status::Invisible {
                offline_idx
            } else {
                hoisted.iter().position(|r| m.roles.contains(&r.id)).unwrap_or(online_idx)
            };
            buckets[slot].push(row);
            count += 1;
        }
        let mut out = vec![];
        for (i, mut rows) in buckets.into_iter().enumerate() {
            if rows.is_empty() {
                continue;
            }
            rows.sort_by_cached_key(|r| r.name.to_lowercase());
            let title = if i < hoisted.len() {
                hoisted[i].name.clone()
            } else if i == online_idx {
                "Online".to_string()
            } else {
                "Offline".to_string()
            };
            out.push(MemberGroup { title, members: rows });
        }
        out
    }

    /// DMs most recent first.
    pub fn dm_list(&self) -> Vec<&Channel> {
        let mut v: Vec<&Channel> = self.private_channels.iter().filter_map(|c| self.channels.get(c)).collect();
        v.sort_by_key(|c| std::cmp::Reverse(c.last_message_id));
        v
    }
}
