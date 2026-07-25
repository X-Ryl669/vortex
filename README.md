# 🌀 Vortex

> **Your devices, one flow.**
> A seamless ecosystem for Linux and Android.

![Status](https://img.shields.io/badge/status-beta-brightgreen)
![Version](https://img.shields.io/badge/version-1.0.0--beta-blue)
![Platform](https://img.shields.io/badge/platform-Linux%20%7C%20Android-blue)
![License](https://img.shields.io/badge/license-GPLv3-blue)

---

## 🧪 Status: Beta

V1 is **feature-complete** — everything listed below works and is in closed
beta. So far it has been live-tested on one device pair (Redmi 9 / MIUI +
Ubuntu / GNOME); widening that coverage is what this beta is for.

---

## 📖 What is this?

Vortex gives your Linux and Android devices the seamless one-ecosystem
feel — notifications, clipboard, calls, audio and files flowing between
them as if they were one device:

- ✅ **Open** — any Android + any Linux
- ✅ **Offline-first** — no cloud, works with no internet at all
- ✅ **Hardware-agnostic** — no premium gear required (budget BT earbuds work)
- ✅ **Privacy-first** — everything end-to-end encrypted, stored locally

## 🎯 Features (working today)

- **Notification mirror** — bidirectional phone↔laptop: dismiss-sync, action
  buttons, inline reply from the laptop
- **Universal clipboard** — text + images sync, with history (Super+V)
- **Smart audio handoff** — earbuds follow your music between devices;
  jump to the phone on a call and come back after
- **Phone companion** — call banner (accept/decline), SMS, contacts,
  recent calls, dialing from the laptop
- **Browsing handoff** — continue the page you're reading on the phone
  with one click on the laptop
- **Proximity lock** — laptop locks when you walk away with the phone,
  unlocks when you're back
- **Notes + To-dos** — one list on both devices, with due-date reminders
- **File sharing** — instant drop-style, Nautilus/Dolphin integration,
  Wi-Fi Direct fast path
- **Emoji-SAS pairing** — easy AND secure: compare 3 emoji, done
- **Experimental:** screen mirroring (both directions) and continuity
  camera (phone → Linux webcam)

## 🛠 Tech stack

| Layer | Stack |
|---|---|
| Android | Kotlin + Jetpack Compose + platform BLE APIs |
| Linux daemon | Rust + Tokio + bluer + zbus |
| Linux GUI | Tauri 2.0 + Vue 3 + TypeScript |
| Shared protocol | Protocol Buffers + Noise Protocol |
| Transport | BLE (signaling) + LAN/Wi-Fi (bulk) |

## 🚀 Install

**Requirements:** Android 10+ phone · Linux with BlueZ (Ubuntu/Debian, Fedora,
Arch, openSUSE) · Bluetooth (BLE) on both devices. Build tools (Rust/Node) are
installed by the script itself.

### 1. Laptop (Linux)

```bash
git clone https://github.com/zoir-dev/vortex && cd vortex
./install_linux.sh
```

One command does everything and **asks nothing** (auto-yes; pass `--ask` if
you want prompts): system libraries (single `sudo` step, distro auto-detected),
build (10–20 min the first time), install to `~/.local/bin`, menu shortcut,
**autostart** (sits in the tray after login), "Share via Vortex" in
Nautilus/Dolphin, and a top-bar "pill" extension on GNOME. Updates use the
same script. On GNOME+Wayland the pill appears after the first logout/login;
on non-GNOME desktops a tray icon is used instead.

**⚠ Secure Boot:** installation won't stop, but the *Experimental*
continuity-camera module (`v4l2loopback`) stays unsigned and may not load.
If you need it, run once: `sudo dpkg-reconfigure v4l2loopback-dkms` (set a
password, then enter it on the blue MOK screen at reboot). Everything else
is independent of this.

### 2. Phone (Android)

**Easiest — with a USB cable.** The one manual prerequisite is **USB
debugging** (Android security requires you to open that door yourself):

1. Settings → About phone → tap **Build number** **7 times** ("You are now
   a developer"; on Xiaomi: tap **MIUI version** 7 times).
2. Settings → System → **Developer options** → **USB debugging** → ON.
3. Plug the cable; accept **"Allow USB debugging?"** on the phone.

Then:

```bash
./install_android.sh
```

The script installs JDK/adb/Android SDK if needed (one `sudo`), builds,
installs to the phone, and enables Notification access + Accessibility +
background clipboard over adb by itself.

**APK route** (no cable/adb): install the release APK from
[Releases](https://github.com/zoir-dev/vortex/releases), then enable two
things manually: Settings → Apps → Special access → **Notification access**
and **Accessibility** → Vortex ✓. (The app also deep-links you to those
pages when needed.)

### 3. Permissions — the phone asks by itself

On first launch the app requests what it needs (Bluetooth/Nearby devices,
notifications, later calls/SMS/contacts) — **grant them all**; each unlocks
one feature. Skip any and the app will remind you later.

### 4. Autostart — the most important manual step! 📌

Many vendors kill background apps; without Autostart, Vortex won't come
back up after a reboot. The app shows a reminder card that opens the right
settings page — you just tick the box:

- **Xiaomi/Redmi/POCO:** Security → Autostart → Vortex ✓
- **Samsung:** Settings → Battery → Never sleeping apps → Vortex
- Other brands: look for "Autostart" / "battery optimization" exceptions in
  your vendor's settings

**Extra on Xiaomi:** without Developer options → **MIUI optimization → OFF**,
calling/SMS from the laptop won't work (MIUI silently blocks those permissions).

### 5. Pairing

1. On the laptop open Vortex (from the tray) → **"Add phone"**; keep the app
   open on the phone.
2. The radar finds the phone → click it → **3 emoji** appear on both screens —
   check they match (that's the security step).
3. Tap **Confirm** on the phone — done. No re-pairing after that; devices
   reconnect on their own (BLE or same Wi-Fi).

⚠ If another BLE phone-link app is running on the laptop, close it during
pairing — they fight over the BLE channel.

## 🔐 Security

All device-to-device traffic is **end-to-end encrypted**:
- Pairing: Noise XX for first pairing, Noise IK for trusted reconnect
- Runtime: ChaCha20-Poly1305
- Key storage: hardware-backed (Android Keystore, Linux Secret Service)

## 💬 Community

- **Telegram:** [t.me/vortexconnect](https://t.me/vortexconnect) — release
  announcements + discussion group (English and Uzbek welcome)
- **GitHub Issues** — bug reports (include phone model + ROM and Linux
  distro + desktop; BLE quirks are very device-specific)
- **GitHub Discussions** — questions and ideas

## 🤝 Contributing

Bug reports and issues are very welcome — especially device/ROM-specific BLE
quirks. PRs: see [CONTRIBUTING.md](CONTRIBUTING.md).

## 📜 License

**[GPLv3](LICENSE)** — forks stay open.

## 🌟 Design philosophy

- **Zero-config UX** — pairing once, everything just flows
- **Noise-based security** — the Signal/WireGuard school: few primitives, used correctly
- **Offline-first** — your data never needs anyone's server

---

**Built with ❤️ for Linux + Android users**
