//! Voice connection handshake: gateway op 4 → VOICE_STATE_UPDATE (session id) +
//! VOICE_SERVER_UPDATE (token, endpoint) → songbird driver.

use std::time::Duration;

use dn_cache::VoiceConn;
use dn_core::{Id, VoiceServerUpdate, VoiceState};
use dn_gateway::{GatewayCommand, VoiceStateUpdate};
use dn_settings::{InputMode, Settings};
use dn_voice::{VoiceConnectInfo, VoiceEvent};

use crate::{Backend, Internal};

#[derive(Debug, Clone)]
pub(crate) struct PendingVoice {
    pub guild_id: Id,
    pub channel_id: Id,
    pub session_id: Option<String>,
    pub token: Option<String>,
    pub endpoint: Option<String>,
    pub connected_with: Option<(String, String, String)>,
}

pub(crate) fn audio_config(s: &Settings) -> dn_audio::AudioConfig {
    dn_audio::AudioConfig {
        input_device: s.voice.input_device.clone(),
        output_device: s.voice.output_device.clone(),
        input_gain: s.voice.input_volume,
        output_volume: s.voice.output_volume,
        mode: match s.voice.input_mode {
            InputMode::VoiceActivity => dn_audio::InputMode::VoiceActivity { threshold_db: s.voice.vad_threshold_db, auto: s.voice.auto_sensitivity },
            InputMode::PushToTalk => dn_audio::InputMode::PushToTalk { release_delay_ms: s.voice.ptt_release_delay_ms },
        },
        noise_suppression: s.voice.noise_suppression,
        auto_gain: s.voice.auto_gain,
    }
}

impl Backend {
    pub(crate) fn join_voice(&mut self, guild_id: Id, channel_id: Id) {
        if self.voice.is_none() {
            self.link.toast("Voice is unavailable: audio is disabled in this build.", true);
            return;
        }
        let (mute, deaf) = {
            let s = self.store();
            (s.voice.self_mute, s.voice.self_deaf)
        };
        let sent = self.gateway_send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
            guild_id,
            channel_id: Some(channel_id),
            self_mute: mute,
            self_deaf: deaf,
        }));
        if !sent {
            self.link.toast("Not connected to Discord.", true);
            return;
        }
        let keep_session = self.voice_pending.as_ref().filter(|p| p.guild_id == guild_id).and_then(|p| p.session_id.clone());
        self.voice_pending = Some(PendingVoice {
            guild_id,
            channel_id,
            session_id: keep_session,
            token: None,
            endpoint: None,
            connected_with: None,
        });
        self.store().voice.conn = VoiceConn::Connecting { guild_id, channel_id };
        self.voice_join_gen += 1;
        let join_gen = self.voice_join_gen;
        let internal = self.internal.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(15)).await;
            let _ = internal.send(Internal::VoiceTimeout(join_gen));
        });
        self.link.repaint.now();
    }

    pub(crate) async fn leave_voice(&mut self) {
        if let Some(p) = self.voice_pending.take() {
            self.gateway_send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
                guild_id: p.guild_id,
                channel_id: None,
                self_mute: false,
                self_deaf: false,
            }));
        }
        if let Some(v) = &self.voice {
            v.disconnect().await;
        }
        let mut s = self.store();
        s.voice.conn = VoiceConn::Idle;
        s.voice.speaking.clear();
        s.voice.self_speaking = false;
        s.voice.ping_ms = None;
        drop(s);
        self.link.repaint.now();
    }

    pub(crate) async fn voice_failed(&mut self, reason: String) {
        tracing::warn!("voice failed: {reason}");
        if let Some(p) = self.voice_pending.take() {
            self.gateway_send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
                guild_id: p.guild_id,
                channel_id: None,
                self_mute: false,
                self_deaf: false,
            }));
        }
        if let Some(v) = &self.voice {
            v.disconnect().await;
        }
        self.store().voice.conn = VoiceConn::Failed(reason.clone());
        self.link.toast(format!("Voice: {reason}"), true);
    }

    /// Re-send our voice state so other clients see mute/deafen changes.
    pub(crate) fn resend_voice_state(&self) {
        let Some(p) = &self.voice_pending else { return };
        let (mute, deaf) = {
            let s = self.store();
            (s.voice.self_mute, s.voice.self_deaf)
        };
        self.gateway_send(GatewayCommand::UpdateVoiceState(VoiceStateUpdate {
            guild_id: p.guild_id,
            channel_id: Some(p.channel_id),
            self_mute: mute,
            self_deaf: deaf,
        }));
    }

    pub(crate) async fn on_own_voice_state(&mut self, v: &VoiceState) {
        match v.channel_id {
            Some(channel) => {
                let Some(p) = self.voice_pending.as_mut() else { return };
                if v.guild_id != Some(p.guild_id) {
                    return;
                }
                p.session_id = Some(v.session_id.clone());
                if p.channel_id != channel {
                    // Moved by a moderator.
                    p.channel_id = channel;
                    let g = p.guild_id;
                    let mut s = self.store();
                    if let Some((_, _)) = s.voice.conn.channel() {
                        s.voice.conn = VoiceConn::Connected { guild_id: g, channel_id: channel };
                    }
                }
                self.try_voice_connect();
            }
            None => {
                if self.voice_pending.is_some() {
                    // Disconnected by a moderator or from another session.
                    self.voice_pending = None;
                    if let Some(vm) = &self.voice {
                        vm.disconnect().await;
                    }
                    let mut s = self.store();
                    s.voice.conn = VoiceConn::Idle;
                    s.voice.speaking.clear();
                }
            }
        }
    }

    pub(crate) fn on_voice_server(&mut self, v: &VoiceServerUpdate) {
        let Some(p) = self.voice_pending.as_mut() else { return };
        if p.guild_id != v.guild_id {
            return;
        }
        p.token = Some(v.token.clone());
        // A null endpoint means the old server went away; a new update follows.
        p.endpoint = v.endpoint.clone().filter(|e| !e.is_empty());
        self.try_voice_connect();
    }

    fn try_voice_connect(&mut self) {
        let Some(me) = self.store().me() else { return };
        let Some(p) = self.voice_pending.as_mut() else { return };
        let (Some(session_id), Some(token), Some(endpoint)) = (p.session_id.clone(), p.token.clone(), p.endpoint.clone()) else { return };
        let key = (session_id.clone(), token.clone(), endpoint.clone());
        if p.connected_with.as_ref() == Some(&key) {
            return;
        }
        p.connected_with = Some(key);
        let info = VoiceConnectInfo { guild_id: p.guild_id, channel_id: p.channel_id, user_id: me, session_id, endpoint, token };
        let Some(voice) = self.voice.clone() else { return };
        let bitrate = self.link.settings.read().voice.bitrate_kbps;
        let internal = self.internal.clone();
        tokio::spawn(async move {
            voice.set_bitrate(bitrate);
            if let Err(e) = voice.connect(info).await {
                let _ = internal.send(Internal::VoiceConnectFailed(e.to_string()));
            }
        });
    }

    pub(crate) fn handle_voice_event(&mut self, ev: VoiceEvent) {
        let target = self.voice_pending.as_ref().map(|p| (p.guild_id, p.channel_id));
        let mut s = self.store();
        match ev {
            VoiceEvent::State(state) => match (state, target) {
                (dn_voice::VoiceState::Connected { .. }, Some((g, c))) => s.voice.conn = VoiceConn::Connected { guild_id: g, channel_id: c },
                (dn_voice::VoiceState::Connecting, Some((g, c))) => s.voice.conn = VoiceConn::Connecting { guild_id: g, channel_id: c },
                (dn_voice::VoiceState::Reconnecting, Some((g, c))) => s.voice.conn = VoiceConn::Reconnecting { guild_id: g, channel_id: c },
                (dn_voice::VoiceState::Failed(e), _) => {
                    s.voice.conn = VoiceConn::Failed(e.clone());
                    drop(s);
                    self.link.toast(format!("Voice connection failed: {e}"), true);
                    let _ = self.internal.send(Internal::VoiceConnectFailed(e));
                    return;
                }
                (dn_voice::VoiceState::Idle, None) => s.voice.conn = VoiceConn::Idle,
                _ => {}
            },
            VoiceEvent::Speaking { user_id, speaking } => {
                if speaking {
                    s.voice.speaking.insert(user_id);
                } else {
                    s.voice.speaking.remove(&user_id);
                }
            }
            VoiceEvent::SelfSpeaking(on) => s.voice.self_speaking = on,
            VoiceEvent::Ping(ms) => s.voice.ping_ms = ms,
            VoiceEvent::UserLeft { user_id } => {
                s.voice.speaking.remove(&user_id);
            }
            VoiceEvent::Error(e) => tracing::warn!("voice: {e}"),
        }
        drop(s);
        self.link.repaint.now();
    }
}
