//! Phone-call event payload (phone → laptop) + control command (laptop →
//! phone).
//!
//! Mirrors an incoming/ongoing phone call to the laptop over the Noise-sealed
//! BLE link (frame type `ty::CALL`) — a continuity-style call banner
//! (ringing → Accept/Decline) followed by an in-call pill (caller + live
//! duration → Mute/End). The control command (`ty::CALL_CONTROL`) carries the
//! laptop's Accept/Decline/End/Mute/… back to the phone. Routed entirely
//! separately from the audio-handoff AUDIO_OP path; never touches the handoff
//! FSM. Mirrors Kotlin `core::call::CallEvent` / `CallControl`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallEvent {
    /// Stable identity for this call across its phase updates.
    #[serde(default)]
    pub id: String,
    /// "ringing" (incoming, unanswered), "active" (connected), "ended".
    #[serde(default)]
    pub phase: String,
    /// Resolved contact display name, or "" when unknown / no permission.
    #[serde(default)]
    pub name: String,
    /// Caller number, or "" when withheld / no permission.
    #[serde(default)]
    pub number: String,
    /// Epoch-millis the call connected (phase=active) so the laptop ticks a
    /// local duration timer without per-second BLE traffic. 0 until answered.
    #[serde(default)]
    pub started_at: i64,
    /// True for an outgoing call.
    #[serde(default)]
    pub outgoing: bool,
    /// True once an OUTGOING call has actually connected (the callee answered) —
    /// detected from the phone dialer's live notification chronometer, since the
    /// TelephonyCallback can't tell dialing from connected. Until then an
    /// outgoing call shows "Calling…" with no timer; once connected the laptop
    /// ticks the duration from `started_at` (the real connect time). Always true
    /// for incoming answered calls (they connect on accept).
    #[serde(default)]
    pub connected: bool,
    /// Default dialer package — the laptop resolves its cached icon so the
    /// banner/pill shows the phone's real Phone-app logo.
    #[serde(default)]
    pub app_id: String,
    /// Epoch-millis (phone clock) this event went on the wire. The laptop
    /// anchors the in-call timer to its OWN clock via
    /// `started_at + (recv_now - sent_at)` so the duration matches the phone
    /// regardless of clock skew. 0 = not stamped (fall back to raw started_at).
    #[serde(default)]
    pub sent_at: i64,
    /// Live in-call audio state (read at send time) for the pill card toggles:
    /// `muted` → Mute/Unmute, `speaker` → Speaker on/off, `has_earbuds` → hide
    /// the Speaker button (buds are on; no loudspeaker needed).
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub speaker: bool,
    #[serde(default)]
    pub has_earbuds: bool,
}

impl CallEvent {
    pub const PHASE_RINGING: &'static str = "ringing";
    pub const PHASE_ACTIVE: &'static str = "active";
    pub const PHASE_ENDED: &'static str = "ended";

    pub fn from_json(bytes: &[u8]) -> Option<Self> {
        let ev: CallEvent = serde_json::from_slice(bytes).ok()?;
        if ev.id.is_empty() || ev.phase.is_empty() {
            return None;
        }
        Some(ev)
    }
}

/// A laptop→phone call-control command (the counterpart to [`CallEvent`]).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CallControl {
    /// Targets the call this command applies to.
    pub id: String,
    /// One of the `action::*` constants.
    pub action: String,
    /// Optional payload (e.g. the SMS body for `action::SMS_REJECT`).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub arg: String,
    /// Monotonic per-command nonce (laptop-set). The phone dedups the SAME
    /// command arriving over both transports by ignoring `seq <= lastSeq`,
    /// while still letting a repeated action through (each click → fresh seq).
    #[serde(default, skip_serializing_if = "is_zero_i64")]
    pub seq: i64,
}

fn is_zero_i64(v: &i64) -> bool {
    *v == 0
}

impl CallControl {
    pub fn to_json(&self) -> Vec<u8> {
        serde_json::to_vec(self).unwrap_or_default()
    }
}

/// Control actions (mirror Kotlin `CallControl.Action`).
pub mod action {
    pub const ACCEPT: &str = "accept";
    pub const DECLINE: &str = "decline";
    pub const END: &str = "end";
    pub const MUTE: &str = "mute";
    pub const UNMUTE: &str = "unmute";
    pub const SPEAKER_ON: &str = "speaker_on";
    pub const SPEAKER_OFF: &str = "speaker_off";
    pub const SILENCE: &str = "silence";
    pub const SMS_REJECT: &str = "sms_reject";
    /// Place a NEW outgoing call from the laptop: `CallControl.id` is empty and
    /// `CallControl.arg` holds the phone number to dial.
    pub const ORIGINATE_CALL: &str = "originate_call";
    /// Send an SMS from the laptop: `CallControl.id` is empty and
    /// `CallControl.arg` is a JSON object `{"to":"<number>","body":"<text>"}`.
    pub const SEND_SMS: &str = "send_sms";
    /// Mark a conversation read on the phone (the laptop opened it):
    /// `CallControl.arg` is `{"thread":<id>,"address":"<number>"}`.
    pub const MARK_READ: &str = "mark_read";
    /// Load one page of a conversation for the laptop's infinite scroll:
    /// `CallControl.arg` is
    /// `{"thread":<id>,"address":"<number>","offset":<n>,"limit":<n>}`.
    pub const LOAD_THREAD: &str = "load_thread";
}
