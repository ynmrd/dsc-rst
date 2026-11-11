/// How the gate decides to open.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GateMode {
    VoiceActivity { threshold_db: f32 },
    PushToTalk { release_delay_ms: u32 },
}

/// Voice-activity / push-to-talk gate evaluated once per audio frame.
///
/// Voice activity opens on the first frame above the threshold and stays open for the
/// hangover after the level drops; push-to-talk stays open for the release delay after the key
/// is released. Mute always forces the gate closed.
#[derive(Debug, Clone)]
pub struct Gate {
    mode: GateMode,
    frame_ms: u32,
    hangover_ms: u32,
    open: bool,
    hold_frames: u32,
}

impl Gate {
    pub const DEFAULT_HANGOVER_MS: u32 = 300;
    pub const DEFAULT_FRAME_MS: u32 = 20;

    pub fn new(mode: GateMode) -> Self {
        Self { mode, frame_ms: Self::DEFAULT_FRAME_MS, hangover_ms: Self::DEFAULT_HANGOVER_MS, open: false, hold_frames: 0 }
    }

    pub fn with_hangover_ms(mut self, hangover_ms: u32) -> Self {
        self.hangover_ms = hangover_ms;
        self
    }

    pub fn with_frame_ms(mut self, frame_ms: u32) -> Self {
        self.frame_ms = frame_ms.max(1);
        self
    }

    pub fn mode(&self) -> GateMode {
        self.mode
    }

    /// Changes parameters without resetting the current open/hold state.
    pub fn set_mode(&mut self, mode: GateMode) {
        self.mode = mode;
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn reset(&mut self) {
        self.open = false;
        self.hold_frames = 0;
    }

    fn frames_for(&self, ms: u32) -> u32 {
        ms.div_ceil(self.frame_ms)
    }

    /// Advances the gate by one frame and returns whether audio should pass.
    pub fn process(&mut self, level_db: f32, ptt_pressed: bool, muted: bool) -> bool {
        if muted {
            self.reset();
            return false;
        }
        let (active, hold_ms) = match self.mode {
            GateMode::VoiceActivity { threshold_db } => (level_db > threshold_db, self.hangover_ms),
            GateMode::PushToTalk { release_delay_ms } => (ptt_pressed, release_delay_ms),
        };
        if active {
            self.open = true;
            self.hold_frames = self.frames_for(hold_ms);
        } else if self.hold_frames > 0 {
            self.hold_frames -= 1;
            self.open = true;
        } else {
            self.open = false;
        }
        self.open
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vad_opens_immediately_and_holds_for_hangover() {
        let mut gate = Gate::new(GateMode::VoiceActivity { threshold_db: -40.0 });
        assert!(!gate.process(-60.0, false, false));
        assert!(gate.process(-30.0, false, false), "attack is one frame");
        // 300 ms hangover at 20 ms frames = 15 frames below threshold still open.
        for i in 0..15 {
            assert!(gate.process(-60.0, false, false), "frame {i} should be held open");
        }
        assert!(!gate.process(-60.0, false, false));
        assert!(!gate.is_open());
    }

    #[test]
    fn vad_hangover_restarts_on_new_speech() {
        let mut gate = Gate::new(GateMode::VoiceActivity { threshold_db: -40.0 }).with_hangover_ms(100);
        assert!(gate.process(-20.0, false, false));
        for _ in 0..3 {
            assert!(gate.process(-80.0, false, false));
        }
        assert!(gate.process(-20.0, false, false));
        for _ in 0..5 {
            assert!(gate.process(-80.0, false, false));
        }
        assert!(!gate.process(-80.0, false, false));
    }

    #[test]
    fn ptt_release_delay() {
        let mut gate = Gate::new(GateMode::PushToTalk { release_delay_ms: 60 });
        assert!(!gate.process(0.0, false, false), "level alone never opens PTT");
        assert!(gate.process(-90.0, true, false));
        for _ in 0..3 {
            assert!(gate.process(-90.0, false, false));
        }
        assert!(!gate.process(-90.0, false, false));

        let mut instant = Gate::new(GateMode::PushToTalk { release_delay_ms: 0 });
        assert!(instant.process(-90.0, true, false));
        assert!(!instant.process(-90.0, false, false));
    }

    #[test]
    fn mute_forces_closed_and_clears_hold() {
        let mut gate = Gate::new(GateMode::VoiceActivity { threshold_db: -40.0 });
        assert!(gate.process(-10.0, false, false));
        assert!(!gate.process(-10.0, false, true));
        assert!(!gate.process(-60.0, false, false), "hangover must not survive a mute");

        let mut ptt = Gate::new(GateMode::PushToTalk { release_delay_ms: 500 });
        assert!(!ptt.process(0.0, true, true));
    }
}
