#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

/// Set the two GTK/WebKit environment variables the app needs, BEFORE anything
/// can initialise GTK.
///
/// They used to live only in the `Exec=` line of the .desktop files that
/// `install_linux.sh` generates. That works when our own installer writes the
/// launcher — and stops working the moment a .deb or .rpm ships a plain
/// `.desktop`, or the user starts the binary from a terminal, which is exactly
/// what packaging this app means. Setting them here makes the binary
/// self-sufficient however it is launched.
///
///  * `GDK_BACKEND=x11` — under GNOME fractional scaling on Wayland, the
///    title-bar buttons ignore the first click. XWayland does not.
///  * `WEBKIT_DISABLE_DMABUF_RENDERER=1` — some GPU stacks render a blank
///    WebKitGTK window without it.
///
/// Both are overridable: if the user set them, theirs win, so anyone wanting
/// to try native Wayland can.
fn set_display_env() {
    for (key, value) in [
        ("GDK_BACKEND", "x11"),
        ("WEBKIT_DISABLE_DMABUF_RENDERER", "1"),
    ] {
        if std::env::var_os(key).is_none() {
            // SAFETY: single-threaded here — nothing has spawned a thread yet,
            // and GTK has not been initialised.
            unsafe { std::env::set_var(key, value) };
        }
    }
}

fn main() {
    set_display_env();
    vortex_ui_tauri_lib::run();
}
