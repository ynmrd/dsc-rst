use std::collections::HashMap;

use dn_audio::FRAME_SAMPLES;
use dn_audio::dsp::soft_clip;
use dn_core::Id;

/// Interleaved stereo samples in one 20 ms voice tick.
pub(crate) const TICK_SAMPLES: usize = FRAME_SAMPLES * 2;

/// Mixes decoded i16 stereo PCM from every speaker into `out` with per-source volume, then
/// soft-clips. Deafened output is silence.
pub(crate) fn mix_tick<'a>(out: &mut [f32], sources: impl IntoIterator<Item = (&'a [i16], f32)>, deafened: bool) {
    out.fill(0.0);
    if deafened {
        return;
    }
    for (pcm, volume) in sources {
        if volume <= 0.0 {
            continue;
        }
        let gain = volume / 32768.0;
        for (o, s) in out.iter_mut().zip(pcm) {
            *o += f32::from(*s) * gain;
        }
    }
    for o in out.iter_mut() {
        *o = soft_clip(*o);
    }
}

/// Whether decoded PCM carries more than near-silence (Discord sends silence frames when a
/// speaker stops; those must not light the speaking indicator).
pub(crate) fn is_audible(pcm: &[i16]) -> bool {
    pcm.iter().any(|s| s.unsigned_abs() > 32)
}

/// SSRC → user mapping learnt from speaking-state updates.
#[derive(Debug, Default)]
pub(crate) struct SsrcMap {
    users: HashMap<u32, Id>,
}

impl SsrcMap {
    /// A user has one audio SSRC per session, so older SSRCs for the same user are replaced.
    pub fn insert(&mut self, ssrc: u32, user: Id) {
        self.users.retain(|s, u| *u != user || *s == ssrc);
        self.users.insert(ssrc, user);
    }

    pub fn user(&self, ssrc: u32) -> Option<Id> {
        self.users.get(&ssrc).copied()
    }

    /// Forgets every SSRC of `user`; returns whether any was known.
    pub fn remove_user(&mut self, user: Id) -> bool {
        let before = self.users.len();
        self.users.retain(|_, u| *u != user);
        self.users.len() != before
    }

    pub fn clear(&mut self) {
        self.users.clear();
    }
}

/// Turns per-tick "heard this user" observations into debounced speaking transitions.
#[derive(Debug)]
pub(crate) struct SpeakingTracker {
    hangover_ticks: u32,
    silent_ticks: HashMap<Id, u32>,
}

impl SpeakingTracker {
    /// 100 ms at 20 ms ticks: bridges packet-loss gaps without making the indicator sluggish.
    pub const DEFAULT_HANGOVER_TICKS: u32 = 5;

    pub fn new(hangover_ticks: u32) -> Self {
        Self { hangover_ticks, silent_ticks: HashMap::new() }
    }

    /// Feeds one tick; returns `(user, speaking)` transitions sorted by user.
    pub fn tick(&mut self, heard: &[Id]) -> Vec<(Id, bool)> {
        let mut changes = Vec::new();
        for &user in heard {
            if self.silent_ticks.insert(user, 0).is_none() {
                changes.push((user, true));
            }
        }
        let hangover = self.hangover_ticks;
        self.silent_ticks.retain(|user, silent| {
            if heard.contains(user) {
                return true;
            }
            *silent += 1;
            if *silent > hangover {
                changes.push((*user, false));
                false
            } else {
                true
            }
        });
        changes.sort_by_key(|(user, _)| *user);
        changes
    }

    /// Drops `user` immediately; returns whether they were speaking.
    pub fn remove(&mut self, user: Id) -> bool {
        self.silent_ticks.remove(&user).is_some()
    }

    /// Drops everyone; returns who was speaking.
    pub fn clear(&mut self) -> Vec<Id> {
        let mut users: Vec<Id> = self.silent_ticks.drain().map(|(u, _)| u).collect();
        users.sort();
        users
    }

    pub fn speaking(&self) -> Vec<Id> {
        let mut users: Vec<Id> = self.silent_ticks.keys().copied().collect();
        users.sort();
        users
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn constant(value: i16) -> Vec<i16> {
        vec![value; TICK_SAMPLES]
    }

    #[test]
    fn mixes_users_with_individual_volume() {
        let a = constant(8192); // 0.25
        let b = constant(-4096); // -0.125
        let mut out = vec![1.0; TICK_SAMPLES];
        mix_tick(&mut out, [(&a[..], 1.0), (&b[..], 2.0)], false);
        assert!(out.iter().all(|&s| s.abs() < 1e-6), "0.25 - 2 * 0.125 = 0");

        mix_tick(&mut out, [(&a[..], 0.5), (&b[..], 0.0)], false);
        assert!(out.iter().all(|&s| (s - 0.125).abs() < 1e-6));
    }

    #[test]
    fn mix_soft_clips_loud_sums() {
        let loud = constant(i16::MAX);
        let mut out = vec![0.0; TICK_SAMPLES];
        mix_tick(&mut out, [(&loud[..], 2.0), (&loud[..], 2.0)], false);
        assert!(out.iter().all(|&s| s > 0.99 && s <= 1.0));
    }

    #[test]
    fn deafened_mix_is_silent() {
        let a = constant(10_000);
        let mut out = vec![0.5; TICK_SAMPLES];
        mix_tick(&mut out, [(&a[..], 1.0)], true);
        assert!(out.iter().all(|&s| s == 0.0));
    }

    #[test]
    fn audibility_ignores_silence_frames() {
        assert!(!is_audible(&constant(0)));
        assert!(!is_audible(&constant(-20)));
        assert!(is_audible(&constant(500)));
    }

    #[test]
    fn speaking_transitions_with_hangover() {
        let (a, b) = (Id(1), Id(2));
        let mut t = SpeakingTracker::new(2);
        assert_eq!(t.tick(&[a]), vec![(a, true)]);
        assert_eq!(t.tick(&[a, b]), vec![(b, true)]);
        assert_eq!(t.speaking(), vec![a, b]);
        assert!(t.tick(&[b]).is_empty());
        assert!(t.tick(&[b]).is_empty());
        assert_eq!(t.tick(&[b]), vec![(a, false)]);
        assert_eq!(t.tick(&[a]), vec![(a, true)]);
        assert!(t.tick(&[]).is_empty());
        assert!(t.tick(&[]).is_empty());
        assert_eq!(t.tick(&[]), vec![(a, false), (b, false)]);
        assert!(t.speaking().is_empty());
    }

    #[test]
    fn speaking_remove_and_clear() {
        let mut t = SpeakingTracker::new(5);
        t.tick(&[Id(3), Id(1)]);
        assert!(t.remove(Id(3)));
        assert!(!t.remove(Id(3)));
        assert_eq!(t.clear(), vec![Id(1)]);
        assert!(t.speaking().is_empty());
    }

    #[test]
    fn ssrc_map_tracks_users_and_disconnects() {
        let mut map = SsrcMap::default();
        map.insert(100, Id(1));
        map.insert(200, Id(2));
        assert_eq!(map.user(100), Some(Id(1)));
        map.insert(101, Id(1));
        assert_eq!(map.user(100), None, "a rejoin replaces the old SSRC");
        assert_eq!(map.user(101), Some(Id(1)));

        assert!(map.remove_user(Id(1)));
        assert!(!map.remove_user(Id(1)));
        assert_eq!(map.user(101), None);
        assert_eq!(map.user(200), Some(Id(2)));
        map.clear();
        assert_eq!(map.user(200), None);
    }
}
