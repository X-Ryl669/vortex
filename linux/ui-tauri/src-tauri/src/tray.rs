//! System-tray icon + menu. Split out of lib.rs; `run()`'s setup hook
//! calls [`setup`] once at startup.

use std::sync::Mutex;

use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    Manager,
};

use vortex_l3_daemon::core::appstate::{AppState, EarbudsInfo};

use crate::{CmdChannel, UiCmd};

/// The tray's battery-readout menu items, kept in managed state so the
/// heartbeat can update their text as batteries change. Two plain `MenuItem`
/// rows (earbuds + phone) with word labels ("Buds"/"Phone") instead of emoji
/// or image icons: the Linux backend (libayatana-appindicator) renders
/// neither custom menu-item icons (IconMenuItem) nor color emoji — both come
/// out blank/invisible on dark themes. Plain text renders in the theme color
/// (visible in dark mode) AND updates live via `set_text`.
pub(crate) struct BatteryMenuItem {
    pub(crate) buds: MenuItem<tauri::Wry>,
    pub(crate) phone: MenuItem<tauri::Wry>,
}

/// Phone-side fields cached from the last inbound AppState, so a local-only
/// refresh (BlueZ rescan — knows nothing about the phone) can still redraw
/// both rows without wiping the phone's data.
struct PhoneSnap {
    name: Option<String>,
    battery: Option<u8>,
    charging: bool,
    earbuds: Option<EarbudsInfo>,
}

static LAST_PHONE: Mutex<Option<PhoneSnap>> = Mutex::new(None);

/// Redraw the tray tooltip + the two battery menu rows. The single render
/// path for all three triggers: inbound phone state over LAN (lan.rs),
/// inbound phone state over BLE (lan_state.rs), and the UI's 5-second local
/// earbuds rescan (cmd_earbuds.rs). The last one is what makes the buds row
/// appear the moment they connect to the laptop, instead of sitting on
/// "Buds --" until the phone's next heartbeat happens to arrive.
pub(crate) fn update_battery_rows(
    app: &tauri::AppHandle,
    local_earbuds: Option<&EarbudsInfo>,
    phone: Option<&AppState>,
) {
    // A fresh phone state refreshes the cache; a local-only refresh reuses it.
    let mut cache = LAST_PHONE.lock().unwrap_or_else(|p| p.into_inner());
    if let Some(p) = phone {
        *cache = Some(PhoneSnap {
            name: p.name.clone(),
            battery: p.battery,
            charging: p.charging,
            earbuds: p.earbuds.clone(),
        });
    }
    let snap = &*cache;

    let pf = |v: Option<u8>| v.map(|x| format!("{x}%")).unwrap_or_else(|| "--".to_string());
    let trunc = |s: &str, max: usize| -> String {
        if s.chars().count() > max {
            let head: String = s.chars().take(max.saturating_sub(3)).collect();
            format!("{}...", head.trim_end())
        } else {
            s.to_string()
        }
    };

    let phone_buds = snap.as_ref().and_then(|s| s.earbuds.as_ref());
    let laptop_owns = local_earbuds.map(|e| e.connected).unwrap_or(false);
    let phone_has = phone_buds.map(|e| e.connected).unwrap_or(false);
    let buds_pct = if laptop_owns {
        local_earbuds.and_then(|e| e.battery)
    } else {
        phone_buds.and_then(|e| e.battery)
    };
    let owner = if laptop_owns {
        "laptop"
    } else if phone_has {
        "phone"
    } else {
        "—"
    };
    let tip = format!(
        "Vortex   🎧 {} ({})   📱 {}",
        pf(buds_pct),
        owner,
        pf(snap.as_ref().and_then(|s| s.battery))
    );
    if let Some(tray) = app.tray_by_id("vortex") {
        let _ = tray.set_tooltip(Some(tip));
    }
    let buds_name = if laptop_owns {
        local_earbuds.map(|e| e.name.clone())
    } else {
        phone_buds.map(|e| e.name.clone())
    }
    .filter(|n| !n.is_empty())
    .or_else(|| vortex_l3_daemon::core::earbuds_store::load().map(|s| s.name))
    .unwrap_or_else(|| "Buds".to_string());
    let buds_text = format!("{}   {} ({})", trunc(&buds_name, 18), pf(buds_pct), owner);
    // ⚡ (U+26A1, present in DejaVu Sans — portable) marks a charging device.
    // No phone seen yet this session → leave the row on its "Phone   --"
    // placeholder rather than inventing a name.
    let phone_text = snap.as_ref().map(|s| {
        let bolt = if s.charging { " \u{26A1}" } else { "" };
        let name = s
            .name
            .clone()
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "Phone".to_string());
        format!("{}   {}{}", trunc(&name, 18), pf(s.battery), bolt)
    });
    drop(cache);
    // Menu mutations must run on the main thread.
    let app_menu = app.clone();
    let _ = app.run_on_main_thread(move || {
        if let Some(item) = app_menu.try_state::<BatteryMenuItem>() {
            let _ = item.buds.set_text(buds_text);
            if let Some(pt) = phone_text {
                let _ = item.phone.set_text(pt);
            }
        }
    });
}

pub(crate) fn setup(app: &tauri::App) -> tauri::Result<()> {
    // System tray (Telegram-style): icon in the top status area;
    // left-click shows/hides the main window; right-click → menu.
    // Battery readout: two disabled rows (earbuds + phone) with plain
    // WORD labels. We deliberately avoid glyph/icon prefixes: color
    // emoji (🎧/📱) render blank in the appindicator menu, and the
    // monochrome icon glyphs that do render here (Font Awesome / Nerd
    // Font PUA, e.g. U+F025/U+F10B) are NOT installed on a stock Linux
    // box, so they'd show empty boxes on other machines. Plain text
    // renders everywhere in the theme color. Refreshed from heartbeat.
    let buds_i = MenuItem::with_id(
        app, "buds_batt", "Buds   --", false, None::<&str>,
    )?;
    let phone_i = MenuItem::with_id(
        app, "phone_batt", "Phone   --", false, None::<&str>,
    )?;
    let switch_i =
        MenuItem::with_id(app, "switch", "Switch earbuds", true, None::<&str>)?;
    let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[&phone_i, &buds_i, &switch_i, &show_i, &quit_i],
    )?;
    app.manage(BatteryMenuItem { buds: buds_i, phone: phone_i });
    // White monochrome BRAND SPIRAL for the status area. Like Telegram / Cursor,
    // we ship ONE fixed light icon rather than swapping per theme: the
    // GNOME/Ubuntu top bar is dark even in light mode, and Linux SNI hosts
    // render a raw PNG as-is (no auto-recolor), so a single white glyph reads
    // everywhere. Embedded via include_bytes so it works from the standalone
    // prod binary.
    let tray_icon = tauri::image::Image::from_bytes(include_bytes!("../icons/tray.png"))
        .unwrap_or_else(|_| app.default_window_icon().unwrap().clone());
    let _ = TrayIconBuilder::with_id("vortex")
        .icon(tray_icon)
        .tooltip("Vortex")
        .menu(&menu)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "switch" => {
                // Toggle the buds between this laptop and the phone.
                use tauri::Manager;
                if let Some(ch) = app.try_state::<CmdChannel>() {
                    let _ = ch.0.send(UiCmd::ToggleEarbuds);
                }
            }
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
        })
        .build(app)?;

    Ok(())
}
