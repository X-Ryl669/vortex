//! Desktop notifications over WinRT toasts — the Windows half of [`Notifier`].
//!
//! # Why this is a thread and not a function
//!
//! Showing a toast is easy; getting the click back is the whole problem. The
//! click arrives as an `Activated` event on the `ToastNotification` object, so
//! *something must keep that object alive* for as long as the toast is on
//! screen — drop it and the button goes nowhere. Toast objects are also not
//! agile, so they cannot be parked in a `Mutex` and touched from whichever tokio
//! worker happens to be free.
//!
//! So one thread owns the notifier, the live toasts, and their event
//! subscriptions; the async trait methods are a channel in front of it. This is
//! the same shape as the advertisement watcher in [`super::ble`], and the same
//! shape `SECRET_RT` uses for libsecret on Linux.
//!
//! # What still has to be true outside this file
//!
//! An unpackaged desktop app has no AppUserModelID until something registers
//! one, and `CreateToastNotifierWithId` with an unregistered AUMID shows
//! *nothing at all* — no error, no toast. The installer must create a
//! Start-menu shortcut carrying [`AUMID`]. That is a packaging job, not a code
//! one, and it is the single most likely reason a Windows build shows no
//! notifications while every call here returns `Ok`.
//!
//! Activation while Vortex is **not running** additionally needs a registered
//! COM activator (`INotificationActivationCallback`). Vortex is a long-running
//! daemon, so in-process `Activated` events cover the cases that matter (file
//! consent, the call banner); a cold-start click is simply lost, which is no
//! worse than the notification never having been shown.
//!
//! None of this has been run — see the note in [`super::ble`].

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Mutex, OnceLock};

use windows::core::{IInspectable, Interface, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastDismissalReason, ToastDismissedEventArgs, ToastNotification,
    ToastNotificationManager,
};

use super::ensure_winrt;
use crate::core::platform::toast_xml::toast_xml;
use crate::core::platform::{BoxFuture, Notifier};

/// The AppUserModelID toasts are shown under. Must match the AUMID on the
/// Start-menu shortcut the installer creates, or nothing appears.
pub const AUMID: &str = "com.vortex.desktop";

/// Where a click lands. Set once by [`Notifier::actions`]; read by the toast
/// thread's event handlers.
static ACTION_SINK: OnceLock<tokio::sync::mpsc::UnboundedSender<(u32, String)>> = OnceLock::new();
/// Where a dismissal lands. Set once by [`Notifier::closures`].
static CLOSURE_SINK: OnceLock<tokio::sync::mpsc::UnboundedSender<(u32, u32)>> = OnceLock::new();

/// Notification ids we hand out. Starts at 1 so 0 keeps its freedesktop
/// meaning of "no id / do not replace".
static NEXT_ID: AtomicU32 = AtomicU32::new(1);

enum Cmd {
    Show {
        summary: String,
        body: String,
        actions: Vec<(String, String)>,
        replaces: u32,
        urgent: bool,
        reply: tokio::sync::oneshot::Sender<Result<u32, String>>,
    },
    Close(u32),
}

fn commands() -> &'static Mutex<Option<Sender<Cmd>>> {
    static TX: OnceLock<Mutex<Option<Sender<Cmd>>>> = OnceLock::new();
    TX.get_or_init(|| Mutex::new(None))
}

/// Hand a command to the toast thread, starting it on first use.
fn send(cmd: Cmd) -> Result<(), String> {
    let mut guard = commands().lock().map_err(|_| "toast channel poisoned")?;
    if guard.is_none() {
        let (tx, rx) = std::sync::mpsc::channel::<Cmd>();
        std::thread::Builder::new()
            .name("vortex-toasts".into())
            .spawn(move || toast_thread(rx))
            .map_err(|e| format!("spawn toast thread: {e}"))?;
        *guard = Some(tx);
    }
    guard
        .as_ref()
        .expect("just set")
        .send(cmd)
        .map_err(|_| "toast thread is gone".to_string())
}

/// Owns the notifier and every live toast, start to finish.
fn toast_thread(rx: Receiver<Cmd>) {
    ensure_winrt();
    let notifier = match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID)) {
        Ok(n) => n,
        Err(e) => {
            // Nothing to fall back to: without a notifier every Show would
            // fail anyway, and the AUMID note at the top of this file is the
            // likely cause.
            tracing::error!("toast notifier unavailable (AUMID registered?): {e}");
            return;
        }
    };
    // id → (toast, tag). The toast is held so its Activated/Dismissed handlers
    // stay alive; the tag is how Windows identifies a toast for replace/remove,
    // since it has no notion of our u32 ids.
    let mut live: HashMap<u32, (ToastNotification, String)> = HashMap::new();

    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Show {
                summary,
                body,
                actions,
                replaces,
                urgent,
                reply,
            } => {
                // Replacing means reusing the previous toast's tag: Windows
                // then updates in place instead of stacking a second banner.
                // That is what the progress notifications rely on.
                let (id, tag) = match live.get(&replaces) {
                    Some((_, tag)) => (replaces, tag.clone()),
                    None => {
                        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
                        (id, format!("vortex-{id}"))
                    }
                };
                let result = show_toast(&notifier, id, &tag, &summary, &body, &actions, urgent);
                match result {
                    Ok(toast) => {
                        live.insert(id, (toast, tag));
                        let _ = reply.send(Ok(id));
                    }
                    Err(e) => {
                        let _ = reply.send(Err(e));
                    }
                }
            }
            Cmd::Close(id) => {
                if let Some((_, tag)) = live.remove(&id) {
                    // Remove by tag, not by object: a toast already dismissed
                    // by the user is gone from history and this is a no-op.
                    if let Ok(history) = ToastNotificationManager::History() {
                        let _ = history.RemoveGroupedTagWithId(
                            &HSTRING::from(tag),
                            &HSTRING::from("vortex"),
                            &HSTRING::from(AUMID),
                        );
                    }
                }
            }
        }
    }
}

fn show_toast(
    notifier: &windows::UI::Notifications::ToastNotifier,
    id: u32,
    tag: &str,
    summary: &str,
    body: &str,
    actions: &[(String, String)],
    urgent: bool,
) -> Result<ToastNotification, String> {
    let xml = XmlDocument::new().map_err(|e| format!("XmlDocument: {e}"))?;
    xml.LoadXml(&HSTRING::from(toast_xml(summary, body, actions, urgent)))
        .map_err(|e| format!("toast xml: {e}"))?;
    let toast = ToastNotification::CreateToastNotification(&xml)
        .map_err(|e| format!("CreateToastNotification: {e}"))?;
    toast
        .SetTag(&HSTRING::from(tag))
        .map_err(|e| format!("SetTag: {e}"))?;
    toast
        .SetGroup(&HSTRING::from("vortex"))
        .map_err(|e| format!("SetGroup: {e}"))?;

    // A click carries the `arguments` of the button pressed — the same
    // `fc:` / `call:` / `act:` keys the Linux path emits, so the routing above
    // this trait needs no Windows-specific branch. A click on the toast BODY
    // has empty arguments and is dropped: the consumers key off a prefix, and
    // "the user clicked somewhere" is not one of their actions.
    let activated = TypedEventHandler::<ToastNotification, IInspectable>::new(
        move |_toast, args| {
            if let Some(args) = args.as_ref() {
                if let Ok(a) = args.cast::<ToastActivatedEventArgs>() {
                    if let Ok(key) = a.Arguments() {
                        let key = key.to_string_lossy();
                        if !key.is_empty() {
                            if let Some(tx) = ACTION_SINK.get() {
                                let _ = tx.send((id, key));
                            }
                        }
                    }
                }
            }
            Ok(())
        },
    );
    toast
        .Activated(&activated)
        .map_err(|e| format!("Activated: {e}"))?;

    // Dismissal reasons are mapped onto the freedesktop `NotificationClosed`
    // codes the existing consumers already interpret (1 expired, 2 dismissed by
    // the user, 3 withdrawn by us, 4 unknown), so a dismissal mirrored back to
    // the phone means the same thing on both platforms.
    let dismissed = TypedEventHandler::<ToastNotification, ToastDismissedEventArgs>::new(
        move |_toast, args| {
            let reason = args
                .as_ref()
                .and_then(|a| a.Reason().ok())
                .map(|r| match r {
                    ToastDismissalReason::TimedOut => 1u32,
                    ToastDismissalReason::UserCanceled => 2,
                    ToastDismissalReason::ApplicationHidden => 3,
                    _ => 4,
                })
                .unwrap_or(4);
            if let Some(tx) = CLOSURE_SINK.get() {
                let _ = tx.send((id, reason));
            }
            Ok(())
        },
    );
    toast
        .Dismissed(&dismissed)
        .map_err(|e| format!("Dismissed: {e}"))?;

    notifier.Show(&toast).map_err(|e| format!("Show: {e}"))?;
    Ok(toast)
}

pub struct WindowsNotifier;

impl Notifier for WindowsNotifier {
    fn show(
        &self,
        summary: &str,
        body: &str,
        _app_id: &str,
        actions: &[(String, String)],
        replaces: u32,
        urgent: bool,
    ) -> BoxFuture<Result<u32, String>> {
        // `app_id` is a freedesktop concept (the desktop-entry name, which is
        // how the shell finds the icon). Windows takes the identity from the
        // AUMID and the icon from that shortcut, so there is nothing to pass it
        // to — see AUMID above.
        // Everything owned up front (the `&str`s do not outlive this call),
        // but the queueing itself happens on await — building a future must
        // not show a notification.
        let summary = summary.to_string();
        let body = body.to_string();
        let actions = actions.to_vec();
        Box::pin(async move {
            let (reply, rx) = tokio::sync::oneshot::channel();
            send(Cmd::Show {
                summary,
                body,
                actions,
                replaces,
                urgent,
                reply,
            })?;
            rx.await
                .map_err(|_| "toast thread dropped the request".to_string())?
        })
    }

    fn close(&self, id: u32) -> BoxFuture<Result<(), String>> {
        Box::pin(async move { send(Cmd::Close(id)) })
    }

    fn actions(&self, tx: tokio::sync::mpsc::UnboundedSender<(u32, String)>) {
        // First caller wins, like the Linux watcher: one process-wide stream
        // that consumers filter by key prefix.
        let _ = ACTION_SINK.set(tx);
    }

    fn closures(&self, tx: tokio::sync::mpsc::UnboundedSender<(u32, u32)>) {
        let _ = CLOSURE_SINK.set(tx);
    }
}
