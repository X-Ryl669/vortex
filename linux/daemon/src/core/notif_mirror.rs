//! Notification mirroring payload (phone → laptop).
//!
//! A phone notification forwarded over the Noise-sealed BLE link (frame
//! type `ty::NOTIFICATION`) and shown as a desktop notification on the
//! laptop. Deliberately tiny — app label + title + text + timestamp, no
//! icon — so it fits a single BLE notify and never logs sensitive content
//! verbatim. The Android side caps the field lengths before sending.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationMirror {
    /// Friendly app label (e.g. "Telegram"), or the package name as a
    /// fallback when the label can't be resolved.
    #[serde(default)]
    pub app: String,
    /// Package name (e.g. "org.telegram.messenger") — a stable id used to look
    /// up this app's cached icon on the laptop. Empty when unknown.
    #[serde(default)]
    pub app_id: String,
    /// Notification title (contentTitle).
    #[serde(default)]
    pub title: String,
    /// Notification body (contentText).
    #[serde(default)]
    pub text: String,
    /// Phone's post timestamp (unix millis). Freshness hint only.
    #[serde(default)]
    pub ts: u64,
    /// Stable notification identity for dismissal sync. Phone→laptop: the
    /// Android `StatusBarNotification.key`. Echoed back in a dismiss message
    /// so the origin side can cancel the matching notification.
    #[serde(default)]
    pub key: String,
    /// When true this is a DISMISS message (not a new notification): the
    /// origin's notification with `key` was cleared, so the peer should
    /// close its mirrored copy. title/text are empty for a dismiss.
    #[serde(default)]
    pub dismiss: bool,
    /// Phone→laptop: labels of the notification's action buttons (e.g.
    /// "Mark as read", "Reply"), in order. Shown as desktop-notification
    /// action buttons. Empty when the notification has no actions.
    #[serde(default)]
    pub actions: Vec<String>,
    /// Phone→laptop: index into `actions` that accepts inline text (a
    /// RemoteInput / reply action), or `-1` if none. The laptop pops a text
    /// entry for this action (GNOME has no inline-reply capability).
    #[serde(default = "neg_one")]
    pub reply_index: i32,
    /// laptop→phone INVOKE message: the index into `actions` the user
    /// clicked on the laptop; the phone fires that action's PendingIntent.
    /// `-1` = not an invoke message.
    #[serde(default = "neg_one")]
    pub invoke_index: i32,
    /// laptop→phone: text for a reply (RemoteInput) action, when the
    /// invoked action accepts inline text. Empty otherwise.
    #[serde(default)]
    pub reply: String,
    /// laptop→phone INVOKE id: a fresh monotonic seq stamped on each invoke so
    /// the phone dedups the SAME invoke arriving over BOTH the BLE NOTIFICATION
    /// frame (fast-path) AND the LAN AppState backstop. 0 = not an invoke.
    #[serde(default)]
    pub seq: u64,
    /// laptop→phone CATCH-UP request: when true this frame is NOT a
    /// notification/dismiss/invoke but a request for the phone to RE-SEND any
    /// currently-active mirrorable notification whose `key` is not already in
    /// `known_keys`. Recovers a phone→laptop notify that was dropped in-air —
    /// BLE Notify is fire-and-forget, so a dropped frame is otherwise lost
    /// forever (the sender still saw a "success"). app/title/text are empty.
    #[serde(default)]
    pub resync: bool,
    /// laptop→phone (with `resync`): the notification keys this laptop has
    /// already displayed, so the phone re-sends ONLY the missing ones — keeping
    /// the catch-up burst minimal (no re-popup of what the user already saw).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub known_keys: Vec<String>,
}

fn neg_one() -> i32 {
    -1
}

impl Default for NotificationMirror {
    fn default() -> Self {
        Self {
            app: String::new(),
            app_id: String::new(),
            title: String::new(),
            text: String::new(),
            ts: 0,
            key: String::new(),
            dismiss: false,
            actions: Vec::new(),
            reply_index: -1,
            invoke_index: -1,
            reply: String::new(),
            seq: 0,
            resync: false,
            known_keys: Vec::new(),
        }
    }
}

impl NotificationMirror {
    /// A laptop-internal marker (never decoded from the wire) meaning "a BLE
    /// frame was just dropped, or the link (re)connected — request a catch-up".
    /// The notif consumer turns it into an outgoing [`resync`] request carrying
    /// the laptop's `known_keys`. Distinguished from a real notification by
    /// `resync == true`.
    pub fn catch_up_signal() -> Self {
        Self { resync: true, ..Self::default() }
    }
}
