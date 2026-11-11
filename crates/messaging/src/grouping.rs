//! Message list grouping: when to show an author header and day dividers.

use chrono::{DateTime, Datelike, Local, TimeZone, Utc};
use dn_core::{Message, message_kind};

pub const GROUP_WINDOW_MS: u64 = 7 * 60 * 1000;

fn date_in<Tz: TimeZone>(m: &Message, tz: &Tz) -> Option<chrono::NaiveDate> {
    let ms = i64::try_from(m.timestamp_ms()).ok()?;
    let utc = DateTime::<Utc>::from_timestamp_millis(ms).filter(|d| (0..=9999).contains(&d.year()))?;
    Some(utc.with_timezone(tz).date_naive())
}

/// Whether `msg` needs its own author header rather than continuing `prev`'s group.
///
/// A new header starts when there is no previous message, the author (or webhook identity)
/// differs, more than seven minutes passed, `msg` is a reply or a non-default kind, `prev` is a
/// system-style message, or the local calendar day changed.
pub fn starts_group(prev: Option<&Message>, msg: &Message) -> bool {
    starts_group_in(&Local, prev, msg)
}

/// [`starts_group`] with an explicit timezone for the day-boundary rule.
pub fn starts_group_in<Tz: TimeZone>(tz: &Tz, prev: Option<&Message>, msg: &Message) -> bool {
    let Some(prev) = prev else { return true };
    if prev.author.id != msg.author.id || prev.webhook_id != msg.webhook_id {
        return true;
    }
    if msg.webhook_id.is_some() && prev.author.username != msg.author.username {
        return true;
    }
    if msg.kind != message_kind::DEFAULT || msg.is_reply() {
        return true;
    }
    if !matches!(prev.kind, message_kind::DEFAULT | message_kind::REPLY) {
        return true;
    }
    if prev.timestamp_ms().abs_diff(msg.timestamp_ms()) > GROUP_WINDOW_MS {
        return true;
    }
    date_in(prev, tz) != date_in(msg, tz)
}

/// Day divider text (`November 11, 2026`) when the local date changes between `prev` and `msg`.
/// The first message (no `prev`) always gets one.
pub fn day_divider(prev: Option<&Message>, msg: &Message) -> Option<String> {
    day_divider_in(&Local, prev, msg)
}

/// [`day_divider`] with an explicit timezone.
pub fn day_divider_in<Tz: TimeZone>(tz: &Tz, prev: Option<&Message>, msg: &Message) -> Option<String> {
    let date = date_in(msg, tz)?;
    if prev.is_some_and(|p| date_in(p, tz) == Some(date)) {
        return None;
    }
    Some(format!("{} {}, {}", month_name(date.month()), date.day(), date.year()))
}

fn month_name(m: u32) -> &'static str {
    const NAMES: [&str; 12] =
        ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    NAMES[(m.clamp(1, 12) - 1) as usize]
}

#[cfg(test)]
mod tests {
    use chrono::FixedOffset;
    use dn_core::{Id, MessageReference, User};

    use super::*;

    fn msg(author: u64, ts: &str) -> Message {
        Message {
            author: User { id: Id(author), username: format!("user{author}"), ..Default::default() },
            timestamp: ts.to_string(),
            ..Default::default()
        }
    }

    const A: &str = "2026-11-11T10:07:00+00:00";

    #[test]
    fn first_message_starts_group() {
        assert!(starts_group_in(&Utc, None, &msg(1, A)));
    }

    #[test]
    fn same_author_within_window_groups() {
        let a = msg(1, A);
        assert!(!starts_group_in(&Utc, Some(&a), &msg(1, "2026-11-11T10:09:00+00:00")));
        assert!(!starts_group_in(&Utc, Some(&a), &msg(1, "2026-11-11T10:14:00+00:00")));
        assert!(!starts_group(Some(&a), &msg(1, "2026-11-11T10:09:00+00:00")));
    }

    #[test]
    fn window_author_and_order() {
        let a = msg(1, A);
        assert!(starts_group_in(&Utc, Some(&a), &msg(1, "2026-11-11T10:14:01+00:00")));
        assert!(starts_group_in(&Utc, Some(&a), &msg(2, "2026-11-11T10:08:00+00:00")));
        let later = msg(1, "2026-11-11T10:20:00+00:00");
        assert!(starts_group_in(&Utc, Some(&later), &a));
    }

    #[test]
    fn replies_and_system_kinds_break_groups() {
        let a = msg(1, A);
        let mut reply = msg(1, "2026-11-11T10:08:00+00:00");
        reply.kind = message_kind::REPLY;
        assert!(starts_group_in(&Utc, Some(&a), &reply));

        let mut referenced = msg(1, "2026-11-11T10:08:00+00:00");
        referenced.message_reference = Some(MessageReference { message_id: Some(Id(9)), ..Default::default() });
        assert!(starts_group_in(&Utc, Some(&a), &referenced));

        let mut system = msg(1, "2026-11-11T10:08:00+00:00");
        system.kind = message_kind::CHANNEL_PINNED_MESSAGE;
        assert!(starts_group_in(&Utc, Some(&a), &system));
        assert!(starts_group_in(&Utc, Some(&system), &msg(1, "2026-11-11T10:09:00+00:00")));

        // A plain follow-up to a reply from the same author stays grouped.
        assert!(!starts_group_in(&Utc, Some(&reply), &msg(1, "2026-11-11T10:09:00+00:00")));
    }

    #[test]
    fn webhooks_group_by_identity() {
        let mut a = msg(5, A);
        a.webhook_id = Some(Id(5));
        let mut b = msg(5, "2026-11-11T10:08:00+00:00");
        b.webhook_id = Some(Id(5));
        assert!(!starts_group_in(&Utc, Some(&a), &b));
        b.author.username = "other name".into();
        assert!(starts_group_in(&Utc, Some(&a), &b));
        let mut c = msg(5, "2026-11-11T10:08:00+00:00");
        c.webhook_id = None;
        assert!(starts_group_in(&Utc, Some(&a), &c));
    }

    #[test]
    fn day_boundary_uses_timezone() {
        let a = msg(1, "2026-11-11T23:58:00+00:00");
        let b = msg(1, "2026-11-12T00:01:00+00:00");
        assert!(starts_group_in(&Utc, Some(&a), &b));
        // Same instants are the same local day at UTC-5.
        let est = FixedOffset::west_opt(5 * 3600).unwrap();
        assert!(!starts_group_in(&est, Some(&a), &b));
    }

    #[test]
    fn day_dividers() {
        let a = msg(1, "2026-11-11T23:58:00+00:00");
        let b = msg(1, "2026-11-12T00:01:00+00:00");
        assert_eq!(day_divider_in(&Utc, None, &a).as_deref(), Some("November 11, 2026"));
        assert_eq!(day_divider_in(&Utc, Some(&a), &b).as_deref(), Some("November 12, 2026"));
        assert_eq!(day_divider_in(&Utc, Some(&a), &msg(2, "2026-11-11T01:00:00+00:00")), None);
        let est = FixedOffset::west_opt(5 * 3600).unwrap();
        assert_eq!(day_divider_in(&est, Some(&a), &b), None);
        assert!(day_divider(None, &a).is_some());
    }

    #[test]
    fn unparsable_timestamp_falls_back_to_snowflake() {
        let mut m = msg(1, "not a date");
        m.id = Id::from_timestamp_ms(1_700_000_000_000);
        assert!(day_divider_in(&Utc, None, &m).is_some());
    }
}
