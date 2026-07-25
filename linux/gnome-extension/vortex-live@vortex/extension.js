// Vortex Live Activities — GNOME Shell extension.
//
// Draws a menu-bar pill (app icon + status) per active live activity,
// expanding on click to a card (status / detail / progress bar).
// Data comes from the Vortex daemon over D-Bus: org.vortex.LiveActivities,
// property `Activities` (JSON array), with PropertiesChanged on every update.

import St from 'gi://St';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';
import Clutter from 'gi://Clutter';
import * as Main from 'resource:///org/gnome/shell/ui/main.js';
import * as PanelMenu from 'resource:///org/gnome/shell/ui/panelMenu.js';
import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

const BUS_NAME = 'org.vortex.LiveActivities';
const OBJ_PATH = '/org/vortex/LiveActivities';
const IFACE = 'org.vortex.LiveActivities1';

export default class VortexLiveExtension extends Extension {
    enable() {
        this._buttons = new Map(); // key -> PanelMenu.Button

        // Re-create the proxy whenever the daemon (re)appears; tear pills down
        // when it goes away.
        this._nameWatch = Gio.bus_watch_name(
            Gio.BusType.SESSION, BUS_NAME, Gio.BusNameWatcherFlags.NONE,
            () => this._connect(),
            () => { this._disconnect(); this._clearAll(); },
        );
    }

    disable() {
        if (this._nameWatch) { Gio.bus_unwatch_name(this._nameWatch); this._nameWatch = 0; }
        this._disconnect();
        this._clearAll();
    }

    _connect() {
        if (this._proxy) return;
        try {
            this._proxy = Gio.DBusProxy.new_for_bus_sync(
                Gio.BusType.SESSION, Gio.DBusProxyFlags.NONE, null,
                BUS_NAME, OBJ_PATH, IFACE, null);
            this._changedId = this._proxy.connect(
                'g-properties-changed', () => this._refresh());
            this._refresh();
        } catch (e) {
            logError(e, 'vortex-live: proxy');
        }
    }

    _disconnect() {
        if (this._proxy && this._changedId) {
            try { this._proxy.disconnect(this._changedId); } catch (e) {}
        }
        this._changedId = 0;
        this._proxy = null;
    }

    _clearAll() {
        if (!this._buttons) return;
        for (const btn of this._buttons.values()) { this._stopTimer(btn); btn.destroy(); }
        this._buttons.clear();
    }

    _refresh() {
        if (!this._proxy) return;
        let json = '[]';
        const v = this._proxy.get_cached_property('Activities');
        if (v) json = v.deepUnpack();
        let list;
        try { list = JSON.parse(json); } catch (e) { list = []; }

        const seen = new Set();
        for (const a of list) {
            if (!a || !a.key) continue;
            seen.add(a.key);
            this._upsert(a);
        }
        for (const [key, btn] of [...this._buttons]) {
            if (!seen.has(key)) { this._stopTimer(btn); btn.destroy(); this._buttons.delete(key); }
        }
    }

    _upsert(a) {
        let btn = this._buttons.get(a.key);
        if (!btn) {
            // menuAlignment 0.5 → the popover arrow sits at the CENTER of the
            // pill, so the expanded card opens centered under its trigger
            // (0.0 anchored the arrow to the pill's left edge, pushing the whole
            // card off to the right).
            btn = new PanelMenu.Button(0.5, 'vortex-live', false);

            // --- panel pill: icon + short status -----------------------------
            const pill = new St.BoxLayout({style_class: 'vortex-pill'});
            btn._icon = new St.Icon({style_class: 'system-status-icon'});
            btn._label = new St.Label({y_align: Clutter.ActorAlign.CENTER, style_class: 'vortex-pill-label'});
            pill.add_child(btn._icon);
            pill.add_child(btn._label);
            btn.add_child(pill);

            // Handoff pill: ONE click opens the page in the default browser, with
            // NO menu/card. Clicking a PanelMenu.Button calls menu.open(), which
            // takes a modal input grab (the "invisible curtain"). So we OVERRIDE
            // open() for the handoff pill to launch the URL instead of ever
            // opening the menu — no grab. `btn._url` (the http(s) URL) is set only
            // for the handoff pill, in the content update below; null otherwise.
            btn._url = null;
            const _origMenuOpen = btn.menu.open.bind(btn.menu);
            btn.menu.open = (animate) => {
                if (btn._url) {
                    try { Gio.AppInfo.launch_default_for_uri(btn._url, null); }
                    catch (e) { logError(e); }
                    return; // never open the menu → no modal grab, no curtain
                }
                _origMenuOpen(animate);
            };

            // --- expanded card -----------------------------------------------
            const card = new St.BoxLayout({vertical: true, style_class: 'vortex-card'});
            const head = new St.BoxLayout({style_class: 'vortex-card-head'});
            btn._cardIcon = new St.Icon({icon_size: 22});
            btn._app = new St.Label({style_class: 'vortex-app', y_align: Clutter.ActorAlign.CENTER});
            head.add_child(btn._cardIcon);
            head.add_child(btn._app);
            btn._title = new St.Label({style_class: 'vortex-title'});
            btn._text = new St.Label({style_class: 'vortex-text'});

            btn._progress = -1;
            btn._bar = new St.DrawingArea({style_class: 'vortex-bar'});
            btn._bar.connect('repaint', (area) => this._drawBar(area, btn._progress));

            btn._sub = new St.Label({style_class: 'vortex-sub'});

            card.add_child(head);
            card.add_child(btn._title);
            card.add_child(btn._text);
            card.add_child(btn._bar);
            card.add_child(btn._sub);

            // In-call action buttons — shown only for the call pill
            // (key 'vortex-call'); clicking sends CallAction(verb) to the
            // daemon → the phone. Labels + verbs are DYNAMIC (set in the
            // content update below): Mute↔Unmute, Speaker on/off, and Speaker
            // is hidden when wireless earbuds are connected.
            btn._callRow = new St.BoxLayout({style_class: 'vortex-call-actions'});
            const mkBtn = () => {
                const b = new St.Button({
                    style_class: 'vortex-call-btn', x_expand: true, can_focus: true,
                });
                b._verb = '';
                b.connect('clicked', () => {
                    if (b._verb) this._callAction(b._verb);
                    btn.menu.close();
                });
                return b;
            };
            btn._muteBtn = mkBtn();
            btn._speakerBtn = mkBtn();
            btn._endBtn = mkBtn();
            btn._endBtn._verb = 'end';
            btn._endBtn.label = 'End';
            btn._callRow.add_child(btn._muteBtn);
            btn._callRow.add_child(btn._speakerBtn);
            btn._callRow.add_child(btn._endBtn);
            card.add_child(btn._callRow);

            // Now-playing transport buttons — shown only for a media pill
            // (activity carries a `playing` flag). Rides the same CallAction
            // channel; the verb carries the player's package name so the
            // phone controls the right session. Unlike the call buttons the
            // card stays OPEN — next/prev are often pressed repeatedly.
            btn._mediaRow = new St.BoxLayout({style_class: 'vortex-call-actions'});
            const mkMediaBtn = (label) => {
                const b = new St.Button({
                    style_class: 'vortex-call-btn', x_expand: true, can_focus: true,
                    label,
                });
                b._verb = '';
                b.connect('clicked', () => { if (b._verb) this._callAction(b._verb); });
                return b;
            };
            btn._prevBtn = mkMediaBtn('⏮');
            btn._playBtn = mkMediaBtn('▶');
            btn._nextBtn = mkMediaBtn('⏭');
            btn._mediaRow.add_child(btn._prevBtn);
            btn._mediaRow.add_child(btn._playBtn);
            btn._mediaRow.add_child(btn._nextBtn);
            card.add_child(btn._mediaRow);

            btn.menu.box.add_child(card);

            // Right box → the pill sits among the system-tray indicators (next
            // to Vortex's own tray icon), not in front of the clock.
            Main.panel.addToStatusArea('vortex-live-' + a.key, btn, 0, 'right');
            this._buttons.set(a.key, btn);
        }

        // --- content -----------------------------------------------------------
        if (a.icon) {
            const g = Gio.icon_new_for_string(a.icon);
            btn._icon.gicon = g;
            btn._cardIcon.gicon = g;
        }
        btn._app.text = a.app || '';
        btn._title.text = a.title || '';
        btn._sub.text = a.sub || '';
        btn._sub.visible = !!(a.sub && a.sub.length);
        btn._progress = (typeof a.progress === 'number') ? a.progress : -1;
        btn._bar.visible = btn._progress >= 0;
        btn._title.visible = !!(a.title && a.title.length);
        // Handoff pill: clicking it opens the page (URL carried in `sub`).
        btn._url = (a.key === 'vortex-handoff' && a.sub) ? a.sub : null;
        // In-call pill action buttons: dynamic from the call audio state.
        btn._callRow.visible = (a.key === 'vortex-call');
        if (a.key === 'vortex-call') {
            // Mute ↔ Unmute toggle.
            const muted = !!a.muted;
            btn._muteBtn.label = muted ? 'Unmute' : 'Mute';
            btn._muteBtn._verb = muted ? 'unmute' : 'mute';
            // Speaker: hidden when earbuds are connected; otherwise on/off.
            const speaker = !!a.speaker;
            btn._speakerBtn.visible = !a.has_earbuds;
            btn._speakerBtn.label = speaker ? 'Speaker off' : 'Speaker';
            btn._speakerBtn._verb = speaker ? 'speaker_off' : 'speaker_on';
        }
        // Now-playing pill: transport row + ⏸/▶ from the live playing flag.
        const isMedia = (a.playing !== undefined && a.playing !== null);
        btn._mediaRow.visible = isMedia;
        if (isMedia) {
            const appId = a.app_id || '';
            btn._playBtn.label = a.playing ? '⏸' : '▶';
            btn._playBtn._verb = 'media_play_pause:' + appId;
            btn._prevBtn._verb = 'media_prev:' + appId;
            btn._nextBtn._verb = 'media_next:' + appId;
        }
        btn._bar.queue_repaint();

        // Duration timer (the in-call pill): the daemon sends `started_at`
        // (epoch-ms the call connected) ONCE and we tick the label LOCALLY,
        // so the daemon needn't republish every second (that starved its
        // D-Bus method dispatch). `started_at` 0 / absent → static text.
        btn._baseText = a.text || '';
        if (typeof a.started_at === 'number' && a.started_at > 0) {
            btn._startedAt = a.started_at;
            this._renderTimed(btn);
            if (!btn._timerId) {
                btn._timerId = GLib.timeout_add(GLib.PRIORITY_DEFAULT, 1000, () => {
                    this._renderTimed(btn);
                    return GLib.SOURCE_CONTINUE;
                });
            }
        } else {
            this._stopTimer(btn);
            // Media pill leads with the TRACK (title); others with the detail.
            const status = (isMedia
                ? (a.title || a.text || a.app || '')
                : (a.text || a.title || a.app || '')).slice(0, 28);
            btn._label.text = ' ' + status;
            btn._text.text = a.text || '';
            btn._text.visible = !!(a.text && a.text.length);
        }
    }

    // Update a timed pill's label/card to "<base> · M:SS" from started_at.
    _renderTimed(btn) {
        const secs = Math.max(0, Math.floor((GLib.get_real_time() / 1000 - btn._startedAt) / 1000));
        const h = Math.floor(secs / 3600), m = Math.floor((secs % 3600) / 60), s = secs % 60;
        const pad = (n) => (n < 10 ? '0' + n : '' + n);
        const dur = h > 0 ? `${h}:${pad(m)}:${pad(s)}` : `${m}:${pad(s)}`;
        const txt = btn._baseText ? `${btn._baseText} · ${dur}` : dur;
        btn._label.text = ' ' + txt.slice(0, 28);
        btn._text.text = txt;
        btn._text.visible = true;
    }

    _stopTimer(btn) {
        if (btn._timerId) {
            GLib.Source.remove(btn._timerId);
            btn._timerId = 0;
        }
    }

    // Invoke an in-call action on the daemon (Mute / Speaker / End) → phone.
    _callAction(verb) {
        if (!this._proxy) return;
        try {
            this._proxy.call('CallAction', new GLib.Variant('(s)', [verb]),
                Gio.DBusCallFlags.NONE, -1, null, null);
        } catch (e) {
            logError(e, 'vortex-live: CallAction');
        }
    }

    _drawBar(area, progress) {
        const cr = area.get_context();
        const [w, h] = area.get_surface_size();
        const r = h / 2;
        const rr = (x0, y0, x1, y1) => {
            cr.newSubPath();
            cr.arc(x1 - r, y0 + r, r, -Math.PI / 2, 0);
            cr.arc(x1 - r, y1 - r, r, 0, Math.PI / 2);
            cr.arc(x0 + r, y1 - r, r, Math.PI / 2, Math.PI);
            cr.arc(x0 + r, y0 + r, r, Math.PI, 1.5 * Math.PI);
            cr.closePath();
        };
        // track
        cr.setSourceRGBA(1, 1, 1, 0.16);
        rr(0, 0, w, h);
        cr.fill();
        // fill (accent green)
        const p = Math.max(0, Math.min(100, progress));
        const fw = Math.max(h, (w * p) / 100);
        cr.setSourceRGBA(0.20, 0.78, 0.35, 1.0);
        rr(0, 0, fw, h);
        cr.fill();
        cr.$dispose();
    }
}
