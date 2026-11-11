//! Bounded per-channel message windows.

use std::collections::VecDeque;

use dn_core::{Id, Message, MessageUpdate, ReactionEvent, Reaction};

/// Messages for one channel in ascending id order. The window covers a contiguous range
/// of history; `reached_start` means nothing older exists and `has_newer_gap` means newer
/// messages exist that are not loaded (the user scrolled far back and the newest end
/// was trimmed to bound memory).
#[derive(Debug, Default, Clone)]
pub struct ChannelMessages {
    pub messages: VecDeque<Message>,
    pub reached_start: bool,
    pub has_newer_gap: bool,
    pub loading_older: bool,
    pub loaded_once: bool,
    /// Bumped on every change so the UI can invalidate layout caches cheaply.
    pub version: u64,
    /// Locally sent messages awaiting the gateway echo, keyed by nonce.
    pub pending: Vec<PendingMessage>,
}

#[derive(Debug, Clone)]
pub struct PendingMessage {
    pub nonce: String,
    pub message: Message,
    pub failed: Option<String>,
}

impl ChannelMessages {
    fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
    }

    pub fn oldest_id(&self) -> Option<Id> {
        self.messages.front().map(|m| m.id)
    }

    pub fn newest_id(&self) -> Option<Id> {
        self.messages.back().map(|m| m.id)
    }

    pub fn get(&self, id: Id) -> Option<&Message> {
        self.position(id).map(|i| &self.messages[i])
    }

    pub fn get_mut(&mut self, id: Id) -> Option<&mut Message> {
        self.position(id).map(move |i| &mut self.messages[i])
    }

    fn position(&self, id: Id) -> Option<usize> {
        // Ascending ids: binary search; fall back to linear for safety.
        let (a, b) = self.messages.as_slices();
        if let Ok(i) = a.binary_search_by(|m| m.id.cmp(&id)) {
            return Some(i);
        }
        if let Ok(i) = b.binary_search_by(|m| m.id.cmp(&id)) {
            return Some(a.len() + i);
        }
        None
    }

    /// Live message from the gateway. Returns false when dropped because the newest end
    /// of the window is not loaded.
    pub fn push_live(&mut self, msg: Message, cap: usize) -> bool {
        if let Some(nonce) = msg.nonce_string() {
            self.pending.retain(|p| p.nonce != nonce);
        }
        if self.has_newer_gap {
            self.bump();
            return false;
        }
        if self.position(msg.id).is_some() {
            return true;
        }
        match self.messages.back() {
            Some(last) if last.id > msg.id => {
                let idx = self.messages.partition_point(|m| m.id < msg.id);
                self.messages.insert(idx, msg);
            }
            _ => self.messages.push_back(msg),
        }
        while self.messages.len() > cap {
            self.messages.pop_front();
            self.reached_start = false;
        }
        self.bump();
        true
    }

    /// Merge a page of history (any order). `older_than` is the cursor used for the request:
    /// Some(id) means "page before id", None means "latest page".
    pub fn merge_page(&mut self, mut page: Vec<Message>, older_than: Option<Id>, reached_start: bool, cap: usize) {
        page.sort_by_key(|m| m.id);
        page.dedup_by_key(|m| m.id);
        self.loading_older = false;
        self.loaded_once = true;
        match older_than {
            None => {
                // Latest page replaces the window unless it overlaps what we have; merging a
                // non-overlapping page would leave an invisible hole in the history.
                let connects = match (page.first(), self.messages.back()) {
                    (Some(first), Some(back)) => first.id <= back.id,
                    _ => false,
                };
                if !connects {
                    self.messages.clear();
                }
                for m in page {
                    if let Some(existing) = self.get_mut(m.id) {
                        *existing = m;
                    } else {
                        let idx = self.messages.partition_point(|x| x.id < m.id);
                        self.messages.insert(idx, m);
                    }
                }
                self.has_newer_gap = false;
                if reached_start {
                    self.reached_start = true;
                }
                while self.messages.len() > cap {
                    self.messages.pop_front();
                    self.reached_start = false;
                }
            }
            Some(_) => {
                for m in page.into_iter().rev() {
                    if self.position(m.id).is_none() {
                        if self.messages.front().is_none_or(|f| m.id < f.id) {
                            self.messages.push_front(m);
                        } else {
                            let idx = self.messages.partition_point(|x| x.id < m.id);
                            self.messages.insert(idx, m);
                        }
                    }
                }
                self.reached_start = reached_start;
                // Scrolling back in time: trim the newest end and remember the gap.
                while self.messages.len() > cap {
                    self.messages.pop_back();
                    self.has_newer_gap = true;
                }
            }
        }
        self.bump();
    }

    pub fn update(&mut self, u: &MessageUpdate) -> bool {
        let changed = match self.get_mut(u.id) {
            Some(m) => {
                u.apply_to(m);
                true
            }
            None => false,
        };
        if changed {
            self.bump();
        }
        changed
    }

    pub fn replace(&mut self, msg: Message) -> bool {
        match self.get_mut(msg.id) {
            Some(m) => {
                *m = msg;
                self.bump();
                true
            }
            None => false,
        }
    }

    pub fn remove(&mut self, ids: &[Id]) -> usize {
        let before = self.messages.len();
        self.messages.retain(|m| !ids.contains(&m.id));
        let removed = before - self.messages.len();
        if removed > 0 {
            self.bump();
        }
        removed
    }

    pub fn reaction_add(&mut self, ev: &ReactionEvent, me: Option<Id>) {
        let Some(m) = self.get_mut(ev.message_id) else { return };
        let is_me = me == Some(ev.user_id);
        match m.reactions.iter_mut().find(|r| r.emoji.same_as(&ev.emoji)) {
            Some(r) => {
                if !(is_me && r.me) {
                    r.count += 1;
                }
                r.me |= is_me;
            }
            None => m.reactions.push(Reaction { count: 1, me: is_me, emoji: ev.emoji.clone() }),
        }
        self.bump();
    }

    pub fn reaction_remove(&mut self, ev: &ReactionEvent, me: Option<Id>) {
        let Some(m) = self.get_mut(ev.message_id) else { return };
        let is_me = me == Some(ev.user_id);
        if let Some(r) = m.reactions.iter_mut().find(|r| r.emoji.same_as(&ev.emoji)) {
            if !(is_me && !r.me) {
                r.count = r.count.saturating_sub(1);
            }
            if is_me {
                r.me = false;
            }
        }
        m.reactions.retain(|r| r.count > 0);
        self.bump();
    }

    pub fn reaction_clear(&mut self, message: Id, emoji: Option<&dn_core::Emoji>) {
        let Some(m) = self.get_mut(message) else { return };
        match emoji {
            Some(e) => m.reactions.retain(|r| !r.emoji.same_as(e)),
            None => m.reactions.clear(),
        }
        self.bump();
    }

    pub fn add_pending(&mut self, nonce: String, message: Message) {
        self.pending.push(PendingMessage { nonce, message, failed: None });
        self.bump();
    }

    pub fn fail_pending(&mut self, nonce: &str, error: String) {
        if let Some(p) = self.pending.iter_mut().find(|p| p.nonce == nonce) {
            p.failed = Some(error);
        }
        self.bump();
    }

    pub fn remove_pending(&mut self, nonce: &str) {
        self.pending.retain(|p| p.nonce != nonce);
        self.bump();
    }

    /// Shrink an inactive channel to its newest `keep` messages.
    pub fn trim_to_newest(&mut self, keep: usize) {
        if self.messages.len() > keep {
            let drop = self.messages.len() - keep;
            self.messages.drain(..drop);
            self.reached_start = false;
            self.bump();
        }
    }
}
