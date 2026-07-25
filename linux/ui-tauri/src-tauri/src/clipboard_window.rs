//! Clipboard-history popup WINDOW management — split out of `clipboard.rs`.
//! Owns the frameless Super+V popup: create/show/hide, focus-loss auto-hide,
//! and the narrow↔wide resize for the detail pane. No coupling to the clipboard
//! store/sync internals — purely Tauri window plumbing.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager};

// One window, macOS-clipboard style: opens NARROW (list only) and WIDENS to
// reveal a right-hand detail pane when the selected entry needs it (image /
// long text), narrowing back otherwise. +2 for the card's 1px border each side.
const LIST_W: f64 = 462.0;
const PREVIEW_W: f64 = 440.0;
const POPUP_H: f64 = 600.0;
const WIDE_W: f64 = LIST_W + PREVIEW_W;

// Hide only when the window has truly lost focus. A short delayed re-check
// tolerates any transient blur the compositor emits.
static LIST_FOCUSED: AtomicBool = AtomicBool::new(false);

fn hide_popup(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("clipboard") {
        let _ = w.hide();
    }
}

/// Force keyboard focus onto the popup via a direct X11 `SetInputFocus`. Tauri's
/// `set_focus()` only ASKS the WM, but under Wayland+XWayland Mutter's
/// focus-stealing prevention denies a focus request lacking a fresh
/// user-activation timestamp — and the Super+V press went to gnome-shell, not to
/// our already-running process. Without real focus the search box never gets the
/// caret AND the window never emits `Focused(false)`, so click-outside can't
/// auto-hide it. Going straight to X bypasses the WM policy. Best-effort: the
/// window maps asynchronously after `show()`, so retry briefly.
fn force_x11_focus() {
    std::thread::spawn(|| {
        use x11rb::connection::Connection;
        use x11rb::protocol::xproto::{
            AtomEnum, ConfigureWindowAux, ConnectionExt, InputFocus, StackMode,
        };

        fn find(conn: &impl Connection, win: u32, target: &str) -> Option<u32> {
            if let Ok(r) =
                conn.get_property(false, win, AtomEnum::WM_NAME, AtomEnum::STRING, 0, 1024)
            {
                if let Ok(r) = r.reply() {
                    if !r.value.is_empty()
                        && String::from_utf8_lossy(&r.value).contains(target)
                    {
                        return Some(win);
                    }
                }
            }
            let tree = conn.query_tree(win).ok()?.reply().ok()?;
            for child in tree.children {
                if let Some(w) = find(conn, child, target) {
                    return Some(w);
                }
            }
            None
        }

        let Ok((conn, screen)) = x11rb::connect(None) else {
            return;
        };
        let root = conn.setup().roots[screen].root;
        for _ in 0..25 {
            std::thread::sleep(std::time::Duration::from_millis(40));
            let Some(win) = find(&conn, root, "Vortex Clipboard") else {
                continue;
            };
            let _ = conn
                .configure_window(win, &ConfigureWindowAux::new().stack_mode(StackMode::ABOVE));
            let _ = conn.set_input_focus(InputFocus::PARENT, win, x11rb::CURRENT_TIME);
            let _ = conn.flush();
            return;
        }
    });
}

fn schedule_group_hide(app: &AppHandle) {
    let app = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(160));
        if !LIST_FOCUSED.load(Ordering::SeqCst) {
            hide_popup(&app);
        }
    });
}

/// Show the frameless clipboard-history popup. Reuses the window across opens
/// (hide on dismiss, show here); re-arms the webview via a direct eval so a
/// just-copied item appears on first open (WebKitGTK pauses hidden-webview JS,
/// so an event emit right after show() would race the webview waking).
pub(crate) fn show_clipboard_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("clipboard") {
        let _ = w.unminimize();
        let _ = w.set_always_on_top(true);
        let _ = w.set_size(tauri::LogicalSize::new(LIST_W, POPUP_H));
        let _ = w.center();
        let _ = w.show();
        let _ = w.set_focus();
        force_x11_focus();
        let _ = w.eval("window.__vortexRearm && window.__vortexRearm()");
        tracing::info!("clipboard popup: shown");
        return;
    }

    match tauri::WebviewWindowBuilder::new(
        app,
        "clipboard",
        tauri::WebviewUrl::App("index.html#/clipboard".into()),
    )
    .title("Vortex Clipboard")
    .inner_size(LIST_W, POPUP_H)
    // Pin the floor at list-width so the compositor honours the narrow request
    // when the detail pane closes (it won't clamp up to content).
    .min_inner_size(LIST_W, POPUP_H)
    .max_inner_size(WIDE_W, POPUP_H)
    .decorations(false)
    // Transparent WINDOW so the page's rounded-corner card cuts cleanly; the
    // card paints a SOLID background, so there's no see-through.
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(true)
    .center()
    .build()
    {
        Ok(win) => {
            tracing::info!("clipboard popup: created");
            let app2 = app.clone();
            win.on_window_event(move |ev| {
                if let tauri::WindowEvent::Focused(f) = ev {
                    LIST_FOCUSED.store(*f, Ordering::SeqCst);
                    if !*f {
                        schedule_group_hide(&app2);
                    }
                }
            });
            force_x11_focus();
        }
        Err(e) => tracing::error!("clipboard popup: build failed: {e}"),
    }
}

#[tauri::command]
pub fn clipboard_hide(app: AppHandle) {
    hide_popup(&app);
}

/// Widen the popup to reveal the right-hand detail pane, or narrow it back to
/// list-only. The list calls this as the selection changes — `visible` is true
/// for entries that need the extra room (image / long text). Same window
/// throughout, so keyboard focus is never disturbed.
#[tauri::command]
pub fn clipboard_set_preview(app: AppHandle, visible: bool) {
    if let Some(w) = app.get_webview_window("clipboard") {
        let width = if visible { WIDE_W } else { LIST_W };
        let _ = w.set_size(tauri::LogicalSize::new(width, POPUP_H));
    }
}
