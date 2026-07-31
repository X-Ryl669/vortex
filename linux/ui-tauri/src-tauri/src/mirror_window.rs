//! The mirror window — ours, not the video sink's.
//!
//! `glimagesink` used to open its own bare X11 window. That left nowhere to put
//! a title or a close button: the WM-drawn one is mis-hit-tested under
//! fractional display scaling (the user had to click it 8-9 times), so the
//! decorations were stripped entirely and the only way out was the app's
//! "Stop sharing" button. Here we own the window instead — a GTK3 toplevel with
//! a client-side header bar, so GTK does the hit-testing and the X lands on the
//! first click at any scale.
//!
//! It also gives the session somewhere to live *before* the first frame: the
//! window opens the moment the user asks for a mirror, showing the Vortex logo
//! and a spinner, and reveals the video when the picture actually arrives.
//! Previously nothing at all appeared for the second or two the handshake takes.
//!
//! The picture is `gtksink`'s own GTK widget, placed in this window. Two
//! measurements picked that over the alternatives, both at 720x1560 on this
//! hardware. Speed: the GL sinks (`gtkglsink`, `glimagesink`) sustain 14-18 fps
//! because the GL context lives on the Intel display GPU while the NVIDIA
//! decoder produces the frames, so every one crosses the bus; `gtksink`
//! sustains ~174. Visibility: XVideo is faster still and was tried first, but
//! it only draws into a TOPLEVEL X window: handed the id of a native child
//! window under XWayland it reports frames happily and paints nothing, which is
//! exactly what left this window sitting on its spinner while the stream ran.
//!
//! Input is read from that widget and handed to `mirror` already normalized to
//! the frame. Going through GTK rather than the sink's own navigation events is
//! also what fixed scrolling, which those events had been reporting with the
//! wrong sign and seven times too large a step.
//!
//! Everything here must run on the GTK main thread, so every entry point that
//! touches widgets is a `MainContext::invoke` — callable from any thread,
//! including tokio workers and GStreamer's bus thread.

use std::cell::RefCell;
use std::sync::atomic::{AtomicU32, Ordering};

use gstreamer as gst;
// NOTE: only gtk's prelude belongs at module scope. GStreamer's rides glib 0.20
// and GTK's rides 0.18, and both define `Cast` — importing them together makes
// every downcast in this file ambiguous. The GStreamer traits are pulled in
// locally, inside the one function that needs them.
use gtk::prelude::*;
use gtk::{gdk, gdk_pixbuf, glib};

/// The Vortex mark, baked in rather than read from disk: the window has to work
/// from a `~/.local/bin` install where the source tree is long gone.
const LOGO_PNG: &[u8] = include_bytes!("../../src/assets/vortex_logo.png");

/// The decoded frame size, for mapping widget pixels back onto the frame.
static VIDEO_W: AtomicU32 = AtomicU32::new(0);
static VIDEO_H: AtomicU32 = AtomicU32::new(0);

/// Live window, owned by the GTK main thread and touched only from it.
struct Win {
    window: gtk::Window,
    loading: gtk::EventBox,
    /// Where the sink's widget goes once the pipeline is built.
    holder: gtk::Box,
    /// The sink's widget, once attached — the surface input is measured against.
    video: Option<gtk::Widget>,
}

thread_local! {
    static WIN: RefCell<Option<Win>> = const { RefCell::new(None) };
}

/// The app's chosen language, as mirrored to `~/.local/share/vortex/voice/lang`
/// by the frontend on every start (see `voice_settings::set_voice_lang`). Read
/// per call — cheap, and it means a language change lands on the next mirror.
fn lang() -> String {
    std::env::var_os("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".local/share/vortex/voice/lang"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_lowercase())
        .filter(|s| matches!(s.as_str(), "en" | "uz" | "ru"))
        .unwrap_or_else(|| "en".to_string())
}

fn connecting_text(lang: &str, name: &str) -> String {
    match lang {
        "uz" => format!("{name}ga ulanilmoqda…"),
        "ru" => format!("Подключение к {name}…"),
        _ => format!("Connecting to {name}…"),
    }
}

fn stop_tooltip(lang: &str) -> &'static str {
    match lang {
        "uz" => "Ekranni uzatishni to'xtatish",
        "ru" => "Остановить трансляцию",
        _ => "Stop mirroring",
    }
}

/// Scale the logo to `px` on its long edge. Returns an empty image if the PNG
/// somehow fails to decode — a missing logo must never stop a mirror.
fn logo_image(px: i32) -> gtk::Image {
    let loader = gdk_pixbuf::PixbufLoader::new();
    let pixbuf = loader
        .write(LOGO_PNG)
        .ok()
        .and_then(|_| loader.close().ok())
        .and_then(|_| loader.pixbuf())
        .and_then(|p| p.scale_simple(px, px, gdk_pixbuf::InterpType::Bilinear));
    match pixbuf {
        Some(p) => gtk::Image::from_pixbuf(Some(&p)),
        None => gtk::Image::new(),
    }
}

/// Dark chrome so the loading page and any letterbox margin match the video
/// rather than flashing the light GTK theme around a black picture.
fn apply_css(window: &gtk::Window) {
    let css = gtk::CssProvider::new();
    let _ = css.load_from_data(
        b"window.vortex-mirror, .vortex-mirror headerbar { background: #16161a; }
          .vortex-mirror headerbar { border: none; box-shadow: none; min-height: 38px; }
          .vortex-mirror .mirror-title { color: #f2f2f7; font-weight: 600; }
          .vortex-mirror .mirror-status { color: #a1a1aa; }
          .vortex-mirror .mirror-stage, .vortex-mirror .mirror-cover { background: #000000; }",
    );
    if let Some(screen) = WidgetExt::screen(window) {
        gtk::StyleContext::add_provider_for_screen(
            &screen,
            &css,
            gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
        );
    }
}

/// Where the frame actually sits inside the widget. XVideo honours the aspect
/// ratio, so a widget that is not exactly the frame's shape leaves a margin;
/// clicks in that margin must not be reported as clicks on the phone's edge.
/// Returns `(x0, y0, w, h)` in widget pixels.
fn video_rect(alloc_w: f64, alloc_h: f64) -> (f64, f64, f64, f64) {
    let vw = VIDEO_W.load(Ordering::Relaxed).max(1) as f64;
    let vh = VIDEO_H.load(Ordering::Relaxed).max(1) as f64;
    let scale = (alloc_w / vw).min(alloc_h / vh);
    let w = (vw * scale).max(1.0);
    let h = (vh * scale).max(1.0);
    (((alloc_w - w) / 2.0), ((alloc_h - h) / 2.0), w, h)
}

/// Widget pixel → the 0..=65535 frame coordinate the phone expects.
fn to_frame(widget: &gtk::Widget, ex: f64, ey: f64) -> (u16, u16) {
    let alloc = widget.allocation();
    let (x0, y0, w, h) = video_rect(alloc.width() as f64, alloc.height() as f64);
    let nx = ((ex - x0) / w * 65535.0).clamp(0.0, 65535.0) as u16;
    let ny = ((ey - y0) / h * 65535.0).clamp(0.0, 65535.0) as u16;
    (nx, ny)
}

/// Open the window in its connecting state. Any previous one is torn down
/// first, so a repeated "mirror" click never stacks two windows.
///
/// `frame_w`/`frame_h` are the DECODED frame size (needed to map input back);
/// `win_w`/`win_h` are how big the window should open.
pub(crate) fn open(title: String, frame_w: u32, frame_h: u32, win_w: i32, win_h: i32) {
    VIDEO_W.store(frame_w, Ordering::Relaxed);
    VIDEO_H.store(frame_h, Ordering::Relaxed);
    glib::MainContext::default().invoke(move || {
        destroy_now();
        let lang = lang();

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title(&title);
        window.set_default_size(win_w.max(240), win_h.max(320));
        window.style_context().add_class("vortex-mirror");
        apply_css(&window);

        // Client-side decorations: GTK draws the header AND hit-tests it, which
        // is the whole point — the server-side close button is off-target under
        // fractional scaling.
        let header = gtk::HeaderBar::new();
        header.set_show_close_button(false);
        let title_label = gtk::Label::new(Some(&title));
        title_label.style_context().add_class("mirror-title");
        header.set_custom_title(Some(&title_label));

        let close_btn =
            gtk::Button::from_icon_name(Some("window-close-symbolic"), gtk::IconSize::Menu);
        close_btn.set_relief(gtk::ReliefStyle::None);
        close_btn.set_tooltip_text(Some(stop_tooltip(&lang)));
        close_btn.set_can_focus(false); // keystrokes belong to the phone
        close_btn.connect_clicked(|_| crate::mirror::request_stop());
        header.pack_end(&close_btn);
        window.set_titlebar(Some(&header));

        // The sink's widget arrives later (the pipeline is built once the
        // session is up), so hold its place with a box.
        let holder = gtk::Box::new(gtk::Orientation::Vertical, 0);
        holder.style_context().add_class("mirror-stage");
        holder.set_hexpand(true);
        holder.set_vexpand(true);

        // The connecting state sits ON TOP of the video rather than swapping
        // with it in a stack, so revealing the picture is one `hide()` and the
        // sink's widget never has to be re-parented mid-stream.
        // An EventBox, not a plain Box: this needs a window of its own so it
        // can be raised above the sink's widget, whatever kind of window that
        // turns out to have.
        let loading = gtk::EventBox::new();
        loading.style_context().add_class("mirror-cover");
        loading.set_valign(gtk::Align::Fill);
        loading.set_halign(gtk::Align::Fill);
        let inner = gtk::Box::new(gtk::Orientation::Vertical, 18);
        inner.set_valign(gtk::Align::Center);
        inner.set_halign(gtk::Align::Center);
        inner.set_vexpand(true);
        inner.add(&logo_image(96));
        let spinner = gtk::Spinner::new();
        spinner.set_size_request(24, 24);
        spinner.start();
        inner.add(&spinner);
        let status = gtk::Label::new(Some(&connecting_text(&lang, &title)));
        status.style_context().add_class("mirror-status");
        inner.add(&status);
        loading.add(&inner);

        let overlay = gtk::Overlay::new();
        overlay.add(&holder);
        overlay.add_overlay(&loading);
        window.add(&overlay);

        // Only a minimum size is hinted. An ASPECT hint used to be set here as
        // well, and it had to go: Mutter does not honour it on a client-side-
        // decorated window (measured — it returned 818x1800 for a 0.4615 frame),
        // and while ignored it still adjusted the WIDTH from the height, which
        // fought the correction below adjusting the height from the width. The
        // two settled on a shape neither wanted. One owner of the constraint.
        let aspect = frame_w.max(1) as f64 / frame_h.max(1) as f64;
        window.set_geometry_hints(
            Some(&holder),
            Some(&gdk::Geometry::new(
                200,
                (200.0 / aspect) as i32,
                -1,
                -1,
                0,
                0,
                0,
                0,
                0.0,
                0.0,
                gdk::Gravity::NorthWest,
            )),
            gdk::WindowHints::MIN_SIZE,
        );

        // Keep the window the phone's shape, so the sink never has to letterbox
        // and no black band appears above or below the picture. Whenever the
        // video area is allocated, check what height its width implies and nudge
        // the window to it — width is what the user drags, height follows. The
        // 2 px tolerance is what keeps this from oscillating: the correction
        // itself triggers another allocation, and without slack the two would
        // trade rounding errors forever.
        {
            let win = window.clone();
            let ratio = frame_w.max(1) as f64 / frame_h.max(1) as f64;
            holder.connect_size_allocate(move |_, alloc| {
                if alloc.width() <= 0 || alloc.height() <= 0 {
                    return;
                }
                let (win_w, win_h) = win.size();
                let chrome_h = win_h - alloc.height(); // header + any border
                let want_h = (alloc.width() as f64 / ratio).round() as i32 + chrome_h;
                if (want_h - win_h).abs() > 2 {
                    win.resize(win_w, want_h);
                }
            });
        }

        wire_keys(&window);

        // Closing from the WM (Alt+F4, the X) must tear the session down in the
        // right order: the pipeline has to reach NULL before the window it
        // draws into goes away. So inhibit the destroy and let the teardown
        // path call `close()` when it is safe.
        window.connect_delete_event(|_, _| {
            crate::mirror::request_stop();
            glib::Propagation::Stop
        });

        window.show_all();

        // Native siblings stack in creation order, which is not a promise —
        // say so explicitly, or the cover can end up behind the picture.
        if let Some(cover) = loading.window() {
            cover.ensure_native();
            cover.raise();
        }

        WIN.with(|w| *w.borrow_mut() = Some(Win { window, loading, holder, video: None }));
    });
}

/// Keys, taken at the WINDOW rather than any one widget: whatever holds focus,
/// every keystroke in this window is meant for the phone. Stopping propagation
/// also keeps Space and Enter from activating the close button.
fn wire_keys(window: &gtk::Window) {
    window.connect_key_press_event(|_, ev| {
        if let Some(name) = ev.keyval().name() {
            crate::mirror::on_key(true, name.as_str());
        }
        glib::Propagation::Stop
    });
    window.connect_key_release_event(|_, ev| {
        if let Some(name) = ev.keyval().name() {
            crate::mirror::on_key(false, name.as_str());
        }
        glib::Propagation::Stop
    });
}

/// Mouse on the sink's widget. Coordinates are converted here, where the widget
/// geometry is known; `mirror` owns everything above that.
fn wire_pointer(video: &gtk::Widget) {
    video.add_events(
        gdk::EventMask::BUTTON_PRESS_MASK
            | gdk::EventMask::BUTTON_RELEASE_MASK
            | gdk::EventMask::POINTER_MOTION_MASK
            | gdk::EventMask::SCROLL_MASK
            | gdk::EventMask::SMOOTH_SCROLL_MASK,
    );
    video.connect_button_press_event(|w, ev| {
        if ev.button() == 1 {
            let (nx, ny) = to_frame(w, ev.position().0, ev.position().1);
            crate::mirror::on_button(true, nx, ny);
        }
        glib::Propagation::Stop
    });
    video.connect_button_release_event(|w, ev| {
        if ev.button() == 1 {
            let (nx, ny) = to_frame(w, ev.position().0, ev.position().1);
            crate::mirror::on_button(false, nx, ny);
        }
        glib::Propagation::Stop
    });
    video.connect_motion_notify_event(|w, ev| {
        let (nx, ny) = to_frame(w, ev.position().0, ev.position().1);
        crate::mirror::on_motion(nx, ny);
        glib::Propagation::Stop
    });
    video.connect_scroll_event(|w, ev| {
        // Discrete wheels report a direction, high-resolution touchpads report
        // deltas. Normalising a notch to +/-1.0 here means one wheel click is
        // one notch of finger travel on the phone, whichever kind of device it
        // is.
        let (dx, dy) = match ev.direction() {
            gdk::ScrollDirection::Up => (0.0, -1.0),
            gdk::ScrollDirection::Down => (0.0, 1.0),
            gdk::ScrollDirection::Left => (-1.0, 0.0),
            gdk::ScrollDirection::Right => (1.0, 0.0),
            _ => ev.delta(),
        };
        let (nx, ny) = to_frame(w, ev.position().0, ev.position().1);
        crate::mirror::on_scroll(nx, ny, dx, dy);
        glib::Propagation::Stop
    });
}

/// Take the sink's widget and put it in the window.
///
/// Must complete BEFORE the pipeline leaves NULL: a gtk sink whose widget has
/// no toplevel parent when it starts creates a window of its own, which would
/// leave a second, bare mirror window on screen. Hence the blocking wait rather
/// than a fire-and-forget invoke. Safe from the GTK thread too, where glib runs
/// the closure inline.
pub(crate) fn attach_video(pipeline: gst::Pipeline) {
    let (done_tx, done_rx) = std::sync::mpsc::channel::<()>();
    glib::MainContext::default().invoke(move || {
        attach_now(&pipeline);
        let _ = done_tx.send(());
    });
    if done_rx
        .recv_timeout(std::time::Duration::from_secs(3))
        .is_err()
    {
        tracing::warn!("mirror-window: widget attach timed out - video may open detached");
    }
}

/// GTK-main-thread half of [`attach_video`]. The sink hands out its widget only
/// from this thread, which is the whole reason for the hop.
fn attach_now(pipeline: &gst::Pipeline) {
    use gst::prelude::*;

    let Some(vsink) = pipeline.by_name("vsink") else {
        tracing::warn!("mirror-window: no vsink - video cannot be embedded");
        return;
    };
    // The tree carries two glib versions: gtk 0.18's (Tauri's GTK3 shell) and
    // gstreamer 0.23's 0.20. They wrap the SAME C objects but are distinct Rust
    // types, so the widget crosses over as a raw GObject pointer.
    // `from_glib_none` takes its own reference while `obj` still holds one, so
    // nothing is stolen from the sink.
    let obj = vsink.property::<gst::glib::Object>("widget");
    let widget: gtk::Widget = unsafe {
        use gst::glib::translate::ToGlibPtr;
        let ptr: *mut gst::glib::gobject_ffi::GObject = obj.to_glib_none().0;
        gtk::glib::translate::from_glib_none(ptr as *mut gtk::ffi::GtkWidget)
    };

    WIN.with(|w| {
        let mut slot = w.borrow_mut();
        let Some(win) = slot.as_mut() else { return };
        for child in win.holder.children() {
            win.holder.remove(&child);
        }
        widget.set_hexpand(true);
        widget.set_vexpand(true);
        widget.set_can_focus(true);
        wire_pointer(&widget);
        win.holder.add(&widget);
        win.holder.show_all();
        // The cover was raised at open time; a widget added later can land above
        // it, so put it back on top until there is a picture worth showing.
        if let Some(cover) = win.loading.window() {
            cover.raise();
        }
        win.video = Some(widget);
    });
}

/// Reveal the picture. Called on the first decoded access unit, so the logo
/// stays up until there is something real to show.
pub(crate) fn show_video() {
    glib::MainContext::default().invoke(|| {
        WIN.with(|w| match w.borrow().as_ref() {
            Some(win) => {
                win.loading.hide();
                if let Some(v) = win.video.as_ref() {
                    v.grab_focus();
                }
                tracing::info!("mirror-window: first frame — revealing the video");
            }
            None => tracing::warn!("mirror-window: first frame but no window is up"),
        });
    });
}

/// Tear the window down. Safe to call repeatedly and from any thread; a no-op
/// when no window is up.
pub(crate) fn close() {
    glib::MainContext::default().invoke(destroy_now);
}

/// GTK-main-thread half of [`close`].
fn destroy_now() {
    WIN.with(|w| {
        if let Some(win) = w.borrow_mut().take() {
            unsafe {
                win.window.destroy();
            }
        }
    });
}
