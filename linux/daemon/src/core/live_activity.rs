//! Live-activity payload (phone → laptop).
//!
//! An ongoing, in-place-updating phone notification (ride ETA, navigation,
//! delivery, timer) forwarded over the Noise-sealed BLE link (frame type
//! `ty::LIVE_ACTIVITY`) and shown as a persistent pill on the laptop's top
//! bar (the system-tray label). Unlike a one-shot notification it carries a
//! stable `key` re-sent on every update, plus an `ended` flag to clear it.

use serde::{Deserialize, Serialize};

fn neg_one() -> i32 {
    -1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LiveActivity {
    /// Stable identity (Android `StatusBarNotification.key`) — the same
    /// across every update so the laptop refreshes one pill in place.
    #[serde(default)]
    pub key: String,
    /// Friendly app label (e.g. "Yandex Go"), package name as a fallback.
    #[serde(default)]
    pub app: String,
    /// Package name — the laptop looks up this app's cached icon for the
    /// live-activity's own tray icon.
    #[serde(default)]
    pub app_id: String,
    /// Short headline (e.g. "Heading your way").
    #[serde(default)]
    pub title: String,
    /// Detail line (e.g. "Arriving in 1 min").
    #[serde(default)]
    pub text: String,
    /// Extra line (sub-text), e.g. "2.3 km · 6 min".
    #[serde(default)]
    pub sub: String,
    /// Progress 0..=100, or `-1` when there's no determinate progress.
    #[serde(default = "neg_one")]
    pub progress: i32,
    /// Epoch-millis a duration counts from (the in-call pill: when the call
    /// connected). `0` = no timer. The GNOME extension ticks it LOCALLY so the
    /// daemon needn't republish every second (which would starve the D-Bus
    /// interface's method dispatch). Phone-sent pills omit it.
    #[serde(default)]
    pub started_at: i64,
    /// In-call pill audio toggles (the call pill only): `muted` → Mute/Unmute,
    /// `speaker` → Speaker on/off, `has_earbuds` → hide the Speaker button.
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub speaker: bool,
    #[serde(default)]
    pub has_earbuds: bool,
    /// True = the activity finished / was removed → clear its pill.
    #[serde(default)]
    pub ended: bool,
    /// Now-playing pill only (mirrors Kotlin `LiveActivity.playing`):
    /// `Some(_)` marks this as the phone's media pill — the extension draws
    /// prev/play-pause/next transport buttons and keys ⏸/▶ off the value.
    /// `None` for every non-media live activity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playing: Option<bool>,
}
