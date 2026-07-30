// Build: android/inject/build.sh — NDK r26b, aarch64, API 29, -O2, which
// reproduces the committed linux/ui-tauri/src-tauri/assets/vortex_inject byte
// for byte (the laptop app embeds that copy with include_bytes!).
//
// Vortex uinput injector — a tiny shell-UID helper that creates a virtual
// multi-touch touchscreen + keyboard via /dev/uinput and injects REAL
// hardware-level events. This bypasses MIUI's `INJECT_EVENTS` block (which kills
// the InputManager / `adb shell input` path) because uinput events enter the
// kernel input subsystem as if from real hardware — no permission required,
// only group access to /dev/uinput (shell is in `net_bt_admin`, its group).
//
// The laptop talks to us over an abstract unix socket ("vortex_inject"), tunneled
// by `adb forward tcp:<port> localabstract:vortex_inject`. A socket (not adb-shell
// stdin) is what keeps the command stream low-latency and un-batched — the same
// reason scrcpy uses one — so scrolling/dragging stay smooth.
//
// Coordinates are NORMALIZED 0..65535 on both axes (resolution-independent);
// Android scales them to the display.
//
// Three devices, deliberately separate: a multi-touch touchscreen, a relative
// pointer (REL_X/REL_Y + buttons + wheel), and a keyboard. Android's InputReader sees a
// real mouse, so it renders + moves its NATIVE on-screen cursor (hover states
// and all), which is what Universal Control needs: the laptop glides its cursor
// onto the phone and drives the phone's own cursor with relative deltas — no
// mirror window, no overlay. The touchscreen device stays for mirror taps.
//
// Protocol (newline-delimited):
//   D <slot> <nx> <ny>   finger down (slot = pointer index, for multitouch)
//   M <slot> <nx> <ny>   finger move
//   U <slot>             finger up
//   E <keycode> <val>    raw key event (val 1=down 0=up) — keyboard
//   K <back|home|recents> navigation key
//   P <dx> <dy>          mouse RELATIVE move (signed) — drives the native cursor
//   B <btn> <val>        mouse button (btn 0=left 1=right 2=middle; val 1=down 0=up)
//   W <dy> [dx]          wheel scroll (vertical, optional horizontal)
//   V 0                  control went back to the laptop: drop the cursor
//                        AND the keyboard, so the phone can raise its own
//   V 1 <ox> <oy> <sx> <sy>  cursor on: slam to the corner (ox,oy) points at,
//                            then step out by (sx,sy) — see mouse_home()
//   Q                    quit
// Test modes (no socket): `vortex_inject tap <nx> <ny>`
//                         `vortex_inject mouse-test`  (wiggles the cursor ~4s)
//                         `vortex_inject keys-hold [secs]`  (keyboard stays up
//                              long enough to answer Android's layout prompt)
// Flags: `--keep-keys`  keep the keyboard device attached between crossings
//                       (only safe when show_ime_with_hard_keyboard is on)
//        `--uhid`       force the UHID backend (cursor + keyboard, no touch);
//                       otherwise it is used only when /dev/uinput is refused

#include <errno.h>
#include <fcntl.h>
#include <linux/uhid.h>
#include <linux/uinput.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <time.h>
#include <unistd.h>

#define ABS_MAX_VAL 65535
#define MAX_SLOTS 10
#define MIN_TAP_MS 40
#define SOCKET_NAME "vortex_inject"
// A freshly created mouse device is not usable the instant UI_DEV_CREATE
// returns. Rather than guess one long settle — which is dead time the user
// spends staring at a pointer parked mid-screen — wait briefly and then re-home
// a few times, ~400 ms of cover in total. Retries stop early enough that they
// barely fight the motion the user has already started.
#define MOUSE_SETTLE_MS 80
#define HOME_RETRIES 6
#define HOME_RETRY_MS 70

static int active_count = 0;

// Whether the keyboard device may stay attached between crossings — set from
// `--keep-keys`, which the laptop passes only when the phone is configured to
// show its on-screen keyboard anyway. See main().
static int keep_keys = 0;
static long slot_down_ms[MAX_SLOTS];

static long now_ms(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return ts.tv_sec * 1000L + ts.tv_nsec / 1000000L;
}

static void emit(int fd, int type, int code, int val) {
    struct input_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.type = type;
    ev.code = code;
    ev.value = val;
    if (write(fd, &ev, sizeof(ev)) < 0) { /* drop one event, keep going */ }
}

static void syn(int fd) { emit(fd, EV_SYN, SYN_REPORT, 0); }


// ── UHID fallback ───────────────────────────────────────────────────────────
//
// /dev/uinput is not ours by right. This phone lets shell open it only because
// Xiaomi ships the node as group net_bt_admin with the uhid_device SELinux
// label, both of which shell happens to hold. Stock Android labels it
// uinput_device and grants shell nothing, and some devices have no such node at
// all — so on most phones the injector would simply have no way in.
//
// /dev/uhid is the sanctioned way. AOSP's shell.te carries exactly one rule for
// input injection — `allow shell uhid_device:chr_file rw_file_perms` — and the
// node exists everywhere, because Bluetooth keyboards and mice are registered
// through it. scrcpy moved to it for the same reasons.
//
// What it cannot do is touch: a HID digitizer is uncharted and scrcpy does not
// attempt one either (it injects touch through the framework, which is the door
// MIUI shuts). So this backend carries the cursor and the keyboard — Universal
// Control — and the touchscreen stays uinput-only.
static int backend_uhid = 0;

static const unsigned char UHID_MOUSE_RDESC[] = {
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01,
    0x95, 0x05, 0x75, 0x01, 0x81, 0x02,                          // 5 buttons
    0x95, 0x01, 0x75, 0x03, 0x81, 0x01,                          // padding
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F,
    0x75, 0x08, 0x95, 0x02, 0x81, 0x06,                          // X, Y (relative)
    0x09, 0x38, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x01,
    0x81, 0x06,                                                  // wheel
    0x05, 0x0C, 0x0A, 0x38, 0x02, 0x15, 0x81, 0x25, 0x7F,
    0x75, 0x08, 0x95, 0x01, 0x81, 0x06,                          // AC Pan (hwheel)
    0xC0, 0xC0,
};

// Boot-protocol keyboard: modifier byte, reserved, then six key slots.
static const unsigned char UHID_KEYS_RDESC[] = {
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01,
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01,
    0x75, 0x01, 0x95, 0x08, 0x81, 0x02,                          // modifiers
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01,                          // reserved
    0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x95, 0x05, 0x75, 0x01,
    0x91, 0x02, 0x95, 0x01, 0x75, 0x03, 0x91, 0x01,              // LEDs
    0x05, 0x07, 0x19, 0x00, 0x29, 0x65, 0x15, 0x00, 0x25, 0x65,
    0x75, 0x08, 0x95, 0x06, 0x81, 0x00,                          // 6 key slots
    0xC0,
};

static int uhid_write_event(int fd, const struct uhid_event *ev) {
    if (fd < 0) return -1;
    if (write(fd, ev, sizeof(*ev)) < 0) {
        fprintf(stderr, "vortex_inject: uhid write failed: %s\n", strerror(errno));
        return -1;
    }
    return 0;
}

static int uhid_create(const char *name, const unsigned char *rd, size_t rd_size,
                       unsigned product) {
    int fd = open("/dev/uhid", O_RDWR | O_CLOEXEC);
    if (fd < 0) {
        fprintf(stderr, "vortex_inject: open /dev/uhid failed: %s\n", strerror(errno));
        return -1;
    }
    struct uhid_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.type = UHID_CREATE2;
    snprintf((char *)ev.u.create2.name, sizeof(ev.u.create2.name), "%s", name);
    memcpy(ev.u.create2.rd_data, rd, rd_size);
    ev.u.create2.rd_size = (unsigned short)rd_size;
    ev.u.create2.bus = BUS_VIRTUAL;
    ev.u.create2.vendor = 0x1209;
    ev.u.create2.product = product;
    ev.u.create2.version = 1;
    if (uhid_write_event(fd, &ev) < 0) {
        close(fd);
        return -1;
    }
    return fd;
}

static int uhid_report(int fd, const unsigned char *data, size_t len) {
    struct uhid_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.type = UHID_INPUT2;
    ev.u.input2.size = (unsigned short)len;
    memcpy(ev.u.input2.data, data, len);
    return uhid_write_event(fd, &ev);
}

static void uhid_destroy(int fd) {
    struct uhid_event ev;
    memset(&ev, 0, sizeof(ev));
    ev.type = UHID_DESTROY;
    uhid_write_event(fd, &ev);
}

// HID reports carry whole state, not edges, so the buttons have to be remembered
// between them.
static unsigned char uhid_mouse_buttons = 0;

static void uhid_mouse_report(int fd, int dx, int dy, int wheel, int hwheel) {
    unsigned char r[5];
    r[0] = uhid_mouse_buttons;
    r[1] = (unsigned char)(signed char)dx;
    r[2] = (unsigned char)(signed char)dy;
    r[3] = (unsigned char)(signed char)wheel;
    r[4] = (unsigned char)(signed char)hwheel;
    uhid_report(fd, r, sizeof(r));
}

// One HID report carries at most +/-127 per axis, and mouse_home() deliberately
// sends five figures to slam the pointer into a corner. Split rather than clamp:
// clamping would break the over-shoot the homing relies on.
static void uhid_mouse_move(int fd, int dx, int dy) {
    while (dx != 0 || dy != 0) {
        int sx = dx > 127 ? 127 : dx < -127 ? -127 : dx;
        int sy = dy > 127 ? 127 : dy < -127 ? -127 : dy;
        uhid_mouse_report(fd, sx, sy, 0, 0);
        dx -= sx;
        dy -= sy;
    }
}

// evdev keycode -> HID Keyboard/Keypad usage. libei hands the laptop evdev
// codes and the uinput path forwards them untouched; HID needs its own numbers.
// 0 means "no usage" — unmapped keys are dropped rather than sent as garbage.
static unsigned char hid_usage_for(int code) {
    static const unsigned char t[] = {
        /*   0 */ 0,    0x29, 0x1E, 0x1F, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25,
        /*  10 */ 0x26, 0x27, 0x2D, 0x2E, 0x2A, 0x2B, 0x14, 0x1A, 0x08, 0x15,
        /*  20 */ 0x17, 0x1C, 0x18, 0x0C, 0x12, 0x13, 0x2F, 0x30, 0x28, 0,
        /*  30 */ 0x04, 0x16, 0x07, 0x09, 0x0A, 0x0B, 0x0D, 0x0E, 0x0F, 0x33,
        /*  40 */ 0x34, 0x35, 0,    0x31, 0x1D, 0x1B, 0x06, 0x19, 0x05, 0x11,
        /*  50 */ 0x10, 0x36, 0x37, 0x38, 0,    0x55, 0,    0x2C, 0x39, 0x3A,
        /*  60 */ 0x3B, 0x3C, 0x3D, 0x3E, 0x3F, 0x40, 0x41, 0x42, 0x43, 0x53,
        /*  70 */ 0x47, 0x5F, 0x60, 0x61, 0x56, 0x5C, 0x5D, 0x5E, 0x57, 0x59,
        /*  80 */ 0x5A, 0x5B, 0x62, 0x63, 0,    0,    0,    0x44, 0x45, 0,
        /*  90 */ 0,    0,    0,    0,    0,    0,    0x58, 0,    0x54, 0,
        /* 100 */ 0,    0,    0x4A, 0x52, 0x4B, 0x50, 0x4F, 0x4D, 0x51, 0x4E,
        /* 110 */ 0x49, 0x4C, 0,    0,    0,    0,    0,    0,    0,    0,
    };
    if (code < 0 || code >= (int)(sizeof(t) / sizeof(t[0]))) return 0;
    return t[code];
}

// Modifier bit for the eight modifier keycodes, or -1.
static int hid_modifier_bit(int code) {
    switch (code) {
        case KEY_LEFTCTRL:   return 0;
        case KEY_LEFTSHIFT:  return 1;
        case KEY_LEFTALT:    return 2;
        case KEY_LEFTMETA:   return 3;
        case KEY_RIGHTCTRL:  return 4;
        case KEY_RIGHTSHIFT: return 5;
        case KEY_RIGHTALT:   return 6;
        case KEY_RIGHTMETA:  return 7;
        default:             return -1;
    }
}

#define HID_KEY_SLOTS 6
static unsigned char uhid_key_mods = 0;
static unsigned char uhid_keys_down[HID_KEY_SLOTS];

static void uhid_key_event(int fd, int code, int val) {
    int bit = hid_modifier_bit(code);
    if (bit >= 0) {
        if (val) uhid_key_mods |= (unsigned char)(1u << bit);
        else uhid_key_mods &= (unsigned char)~(1u << bit);
    } else {
        unsigned char usage = hid_usage_for(code);
        if (!usage) return;
        if (val) {
            int free_slot = -1;
            for (int i = 0; i < HID_KEY_SLOTS; i++) {
                if (uhid_keys_down[i] == usage) { free_slot = -1; break; }
                if (uhid_keys_down[i] == 0 && free_slot < 0) free_slot = i;
            }
            if (free_slot >= 0) uhid_keys_down[free_slot] = usage;
        } else {
            for (int i = 0; i < HID_KEY_SLOTS; i++) {
                if (uhid_keys_down[i] == usage) uhid_keys_down[i] = 0;
            }
        }
    }
    unsigned char r[8];
    memset(r, 0, sizeof(r));
    r[0] = uhid_key_mods;
    for (int i = 0; i < HID_KEY_SLOTS; i++) r[2 + i] = uhid_keys_down[i];
    uhid_report(fd, r, sizeof(r));
}


// Tear down whichever kind of device this fd is.
static void destroy_device(int fd) {
    if (fd < 0) return;
    if (backend_uhid) uhid_destroy(fd);
    else ioctl(fd, UI_DEV_DESTROY);
    close(fd);
}

static int setup_device(void) {
    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) {
        fprintf(stderr, "vortex_inject: open /dev/uinput failed: %s\n", strerror(errno));
        return -1;
    }

    ioctl(fd, UI_SET_EVBIT, EV_SYN);
    ioctl(fd, UI_SET_EVBIT, EV_KEY);
    ioctl(fd, UI_SET_KEYBIT, BTN_TOUCH);
    // Deliberately NO alphabetic keys here — see setup_key_device().
    ioctl(fd, UI_SET_EVBIT, EV_ABS);
    ioctl(fd, UI_SET_ABSBIT, ABS_X);
    ioctl(fd, UI_SET_ABSBIT, ABS_Y);
    ioctl(fd, UI_SET_ABSBIT, ABS_MT_SLOT);
    ioctl(fd, UI_SET_ABSBIT, ABS_MT_TRACKING_ID);
    ioctl(fd, UI_SET_ABSBIT, ABS_MT_POSITION_X);
    ioctl(fd, UI_SET_ABSBIT, ABS_MT_POSITION_Y);
    ioctl(fd, UI_SET_PROPBIT, INPUT_PROP_DIRECT);

    struct uinput_user_dev uidev;
    memset(&uidev, 0, sizeof(uidev));
    snprintf(uidev.name, UINPUT_MAX_NAME_SIZE, "vortex-touch");
    uidev.id.bustype = BUS_VIRTUAL;
    uidev.id.vendor = 0x1209;
    uidev.id.product = 0x5678;
    uidev.id.version = 1;
    uidev.absmin[ABS_X] = 0;
    uidev.absmax[ABS_X] = ABS_MAX_VAL;
    uidev.absmin[ABS_Y] = 0;
    uidev.absmax[ABS_Y] = ABS_MAX_VAL;
    uidev.absmin[ABS_MT_POSITION_X] = 0;
    uidev.absmax[ABS_MT_POSITION_X] = ABS_MAX_VAL;
    uidev.absmin[ABS_MT_POSITION_Y] = 0;
    uidev.absmax[ABS_MT_POSITION_Y] = ABS_MAX_VAL;
    uidev.absmin[ABS_MT_SLOT] = 0;
    uidev.absmax[ABS_MT_SLOT] = MAX_SLOTS - 1;
    uidev.absmin[ABS_MT_TRACKING_ID] = 0;
    uidev.absmax[ABS_MT_TRACKING_ID] = 65535;

    if (write(fd, &uidev, sizeof(uidev)) < 0) {
        fprintf(stderr, "vortex_inject: write uidev failed: %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    if (ioctl(fd, UI_DEV_CREATE) < 0) {
        fprintf(stderr, "vortex_inject: UI_DEV_CREATE failed: %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    usleep(700 * 1000); // let Android's InputReader notice + map the device
    return fd;
}

// Create the keyboard. Its OWN device, and a short-lived one: Android hides the
// on-screen keyboard for as long as any device with alphabetic keys is present,
// so bundling these into the touchscreen (as this once did) left the phone
// unable to raise its own keyboard at all — tap a text field by hand and
// nothing comes up, because a "hardware keyboard" it cannot see is plugged in.
//
// So the keys live here, appear when the laptop takes control, and go away when
// it hands control back.
static int setup_key_device(void) {
    if (backend_uhid) {
        int fd = uhid_create("vortex-keyboard", UHID_KEYS_RDESC, sizeof(UHID_KEYS_RDESC), 0x567A);
        if (fd >= 0) usleep(MOUSE_SETTLE_MS * 1000);
        return fd;
    }
    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) {
        fprintf(stderr, "vortex_inject: open /dev/uinput (keys) failed: %s\n", strerror(errno));
        return -1;
    }

    ioctl(fd, UI_SET_EVBIT, EV_SYN);
    ioctl(fd, UI_SET_EVBIT, EV_KEY);
    // The whole standard range, so the laptop can send any keystroke: letters,
    // digits, punctuation, enter/backspace/arrows.
    for (int code = 1; code <= 255; code++) ioctl(fd, UI_SET_KEYBIT, code);
    ioctl(fd, UI_SET_KEYBIT, KEY_BACK);
    ioctl(fd, UI_SET_KEYBIT, KEY_HOMEPAGE);
    ioctl(fd, UI_SET_KEYBIT, KEY_APPSELECT);

    struct uinput_user_dev uidev;
    memset(&uidev, 0, sizeof(uidev));
    snprintf(uidev.name, UINPUT_MAX_NAME_SIZE, "vortex-keyboard");
    uidev.id.bustype = BUS_VIRTUAL;
    uidev.id.vendor = 0x1209;
    uidev.id.product = 0x567A;
    uidev.id.version = 1;

    if (write(fd, &uidev, sizeof(uidev)) < 0) {
        fprintf(stderr, "vortex_inject: write uidev (keys) failed: %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    if (ioctl(fd, UI_DEV_CREATE) < 0) {
        fprintf(stderr, "vortex_inject: UI_DEV_CREATE (keys) failed: %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    usleep(MOUSE_SETTLE_MS * 1000);
    return fd;
}

// Create the relative-pointer (mouse) uinput device. Separate from the
// touchscreen so Android's InputReader classifies each cleanly: a device mixing
// ABS_MT (direct touch) with REL_X/REL_Y (mouse) confuses the classifier, so
// scrcpy and we keep them apart. REL_X/REL_Y + BTN_LEFT is the canonical "mouse"
// shape that makes Android show and move its native cursor.
// `settle_ms` is how long we wait for Android's InputReader to notice and map
// the new device. It is also, unavoidably, how long the freshly-added pointer
// sits at the display centre where Android parks it — the caller's first move
// command can only land after this. Keep it as short as the mapping tolerates.
static int setup_mouse_device(int settle_ms) {
    if (backend_uhid) {
        int fd = uhid_create("vortex-mouse", UHID_MOUSE_RDESC, sizeof(UHID_MOUSE_RDESC), 0x5679);
        if (fd >= 0) usleep(settle_ms * 1000);
        return fd;
    }
    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) {
        fprintf(stderr, "vortex_inject: open /dev/uinput (mouse) failed: %s\n", strerror(errno));
        return -1;
    }

    ioctl(fd, UI_SET_EVBIT, EV_SYN);
    ioctl(fd, UI_SET_EVBIT, EV_KEY);
    ioctl(fd, UI_SET_KEYBIT, BTN_LEFT);
    ioctl(fd, UI_SET_KEYBIT, BTN_RIGHT);
    ioctl(fd, UI_SET_KEYBIT, BTN_MIDDLE);
    ioctl(fd, UI_SET_EVBIT, EV_REL);
    ioctl(fd, UI_SET_RELBIT, REL_X);
    ioctl(fd, UI_SET_RELBIT, REL_Y);
    ioctl(fd, UI_SET_RELBIT, REL_WHEEL);
    ioctl(fd, UI_SET_RELBIT, REL_HWHEEL);

    struct uinput_user_dev uidev;
    memset(&uidev, 0, sizeof(uidev));
    snprintf(uidev.name, UINPUT_MAX_NAME_SIZE, "vortex-mouse");
    uidev.id.bustype = BUS_VIRTUAL;
    uidev.id.vendor = 0x1209;
    uidev.id.product = 0x5679;
    uidev.id.version = 1;

    if (write(fd, &uidev, sizeof(uidev)) < 0) {
        fprintf(stderr, "vortex_inject: write uidev (mouse) failed: %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    if (ioctl(fd, UI_DEV_CREATE) < 0) {
        fprintf(stderr, "vortex_inject: UI_DEV_CREATE (mouse) failed: %s\n", strerror(errno));
        close(fd);
        return -1;
    }
    usleep(settle_ms * 1000); // let Android's InputReader notice + map the device
    return fd;
}

static void mouse_move(int fd, int dx, int dy) {
    if (fd < 0) return;
    if (backend_uhid) { uhid_mouse_move(fd, dx, dy); return; }
    if (dx != 0) emit(fd, EV_REL, REL_X, dx);
    if (dy != 0) emit(fd, EV_REL, REL_Y, dy);
    syn(fd);
}

// Park the pointer by slamming it into a screen CORNER and then stepping out.
// The slam is a deliberate massive over-shoot: Android clamps it, so it lands
// on that corner whatever the pointer was doing before — an absolute reset
// built out of relative events. That makes this IDEMPOTENT, which is the point:
// a freshly created device is not mapped by InputReader for some tens of ms and
// silently drops what we send, so the caller fires this several times and lets
// the first one that survives win.
//
// Which corner matters. Android runs relative deltas through a velocity-based
// acceleration curve, so the step-out lands somewhere near, not exactly on, its
// target. The caller therefore picks the corner that sits ON the edge the
// cursor is entering from, and steps out only ALONG that edge: the axis the
// laptop's return logic reads is then exact by construction (it is the clamp),
// and only the harmless one absorbs the acceleration error.
static void mouse_home(int fd, int ox, int oy, int sx, int sy) {
    if (fd < 0) return;
    mouse_move(fd, ox, oy);
    mouse_move(fd, sx, sy);
}

static void mouse_button(int fd, int btn, int val) {
    if (fd < 0) return;
    if (backend_uhid) {
        unsigned char bit = (unsigned char)(1u << (btn == 1 ? 1 : btn == 2 ? 2 : 0));
        if (val) uhid_mouse_buttons |= bit;
        else uhid_mouse_buttons &= (unsigned char)~bit;
        uhid_mouse_report(fd, 0, 0, 0, 0);
        return;
    }
    int code = btn == 1 ? BTN_RIGHT : btn == 2 ? BTN_MIDDLE : BTN_LEFT;
    emit(fd, EV_KEY, code, val);
    syn(fd);
}

static void mouse_wheel(int fd, int dy, int dx) {
    if (fd < 0) return;
    if (backend_uhid) {
        int wy = dy > 127 ? 127 : dy < -127 ? -127 : dy;
        int wx = dx > 127 ? 127 : dx < -127 ? -127 : dx;
        uhid_mouse_report(fd, 0, 0, wy, wx);
        return;
    }
    if (dy != 0) emit(fd, EV_REL, REL_WHEEL, dy);
    if (dx != 0) emit(fd, EV_REL, REL_HWHEEL, dx);
    syn(fd);
}

static void touch_down(int fd, int slot, int nx, int ny) {
    if (fd < 0) return;
    emit(fd, EV_ABS, ABS_MT_SLOT, slot);
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, slot + 1);
    emit(fd, EV_ABS, ABS_MT_POSITION_X, nx);
    emit(fd, EV_ABS, ABS_MT_POSITION_Y, ny);
    emit(fd, EV_ABS, ABS_X, nx);
    emit(fd, EV_ABS, ABS_Y, ny);
    if (active_count == 0) emit(fd, EV_KEY, BTN_TOUCH, 1);
    active_count++;
    if (slot >= 0 && slot < MAX_SLOTS) slot_down_ms[slot] = now_ms();
    syn(fd);
}

static void touch_move(int fd, int slot, int nx, int ny) {
    if (fd < 0) return;
    emit(fd, EV_ABS, ABS_MT_SLOT, slot);
    emit(fd, EV_ABS, ABS_MT_POSITION_X, nx);
    emit(fd, EV_ABS, ABS_MT_POSITION_Y, ny);
    emit(fd, EV_ABS, ABS_X, nx);
    emit(fd, EV_ABS, ABS_Y, ny);
    syn(fd);
}

static void touch_up(int fd, int slot) {
    if (fd < 0) return;
    if (slot >= 0 && slot < MAX_SLOTS) {
        long held = now_ms() - slot_down_ms[slot];
        if (held >= 0 && held < MIN_TAP_MS) usleep((MIN_TAP_MS - held) * 1000);
    }
    emit(fd, EV_ABS, ABS_MT_SLOT, slot);
    emit(fd, EV_ABS, ABS_MT_TRACKING_ID, -1);
    if (active_count > 0) active_count--;
    if (active_count == 0) emit(fd, EV_KEY, BTN_TOUCH, 0);
    syn(fd);
}

static void key_event(int fd, int code, int val) {
    if (fd < 0) return;
    if (backend_uhid) { uhid_key_event(fd, code, val); return; }
    emit(fd, EV_KEY, code, val);
    syn(fd);
}

static void key_tap(int fd, int code) {
    key_event(fd, code, 1);
    key_event(fd, code, 0);
}

static void process_line(int fd, int *mfd, int *kfd, char *line) {
    char c = line[0];
    int a, b, d, e, f, n;
    if (c == 'D' && sscanf(line + 1, "%d %d %d", &a, &b, &d) == 3) {
        touch_down(fd, a, b, d);
    } else if (c == 'M' && sscanf(line + 1, "%d %d %d", &a, &b, &d) == 3) {
        touch_move(fd, a, b, d);
    } else if (c == 'U' && sscanf(line + 1, "%d", &a) == 1) {
        touch_up(fd, a);
    } else if (c == 'P' && sscanf(line + 1, "%d %d", &a, &b) == 2) {
        mouse_move(*mfd, a, b);
    } else if (c == 'B' && sscanf(line + 1, "%d %d", &a, &b) == 2) {
        mouse_button(*mfd, a, b);
    } else if (c == 'W') {
        b = 0;
        if (sscanf(line + 1, "%d %d", &a, &b) >= 1) mouse_wheel(*mfd, a, b);
    } else if (c == 'V' && (n = sscanf(line + 1, "%d %d %d %d %d", &a, &b, &d, &e, &f)) >= 1) {
        // Cursor presence. Android paints a pointer for as long as a mouse
        // device exists, so the only way to show NO cursor while control is
        // back on the laptop is to have no device: destroy on the way out
        // (V 0), recreate on the way in (V 1). Recreating eagerly at V 0 time
        // seemed tidier — it hides the InputReader settle — but Android parks
        // a newly added pointer at the display CENTRE and paints it there
        // immediately, which left a stray cursor sitting in the middle of the
        // phone. So the create has to happen on entry, where the caller's
        // slam immediately drags it off to the edge it came in from.
        if (a == 0) {
            if (*mfd >= 0) {
                // Let go of anything held first: destroying the device with a
                // button down strands it down, and the matching release event
                // arrives after the device is gone (a no-op). Returning to the
                // laptop mid-drag would otherwise leave the phone stuck in one.
                mouse_button(*mfd, 0, 0);
                mouse_button(*mfd, 1, 0);
                mouse_button(*mfd, 2, 0);
                if (backend_uhid) uhid_destroy(*mfd);
                else ioctl(*mfd, UI_DEV_DESTROY);
                close(*mfd);
                *mfd = -1;
            }
            // The keyboard only goes if it has to. Cycling it is what makes
            // Android ask about a keyboard layout on every single crossing, so
            // it is done only where the alternative is worse: on a phone that
            // hides its own keyboard whenever ours is attached. See main().
            if (!keep_keys && *kfd >= 0) {
                if (backend_uhid) uhid_destroy(*kfd);
                else ioctl(*kfd, UI_DEV_DESTROY);
                close(*kfd);
                *kfd = -1;
            }
        } else if (n == 5) {
            if (*mfd < 0) *mfd = setup_mouse_device(MOUSE_SETTLE_MS);
            // Retry the homing: InputReader needs a while to map a brand-new
            // device and drops everything until it has. A single move here got
            // swallowed, which left the pointer sitting where Android parks it
            // — mid-screen — and, worse, left the laptop's dead reckoning
            // convinced it was at the edge, so the smallest nudge bounced
            // control straight back. Homing is idempotent, so retrying costs
            // nothing but the few queued deltas it overrides.
            for (int i = 0; i < HOME_RETRIES; i++) {
                mouse_home(*mfd, b, d, e, f);
                usleep(HOME_RETRY_MS * 1000);
            }
        }
    } else if (c == 'E' && sscanf(line + 1, "%d %d", &a, &b) == 2) {
        if (*kfd < 0) *kfd = setup_key_device();
        key_event(*kfd, a, b);
    } else if (c == 'K') {
        char what[16];
        if (sscanf(line + 1, "%15s", what) == 1) {
            if (*kfd < 0) *kfd = setup_key_device();
            if (strcmp(what, "back") == 0) key_tap(*kfd, KEY_BACK);
            else if (strcmp(what, "home") == 0) key_tap(*kfd, KEY_HOMEPAGE);
            else if (strcmp(what, "recents") == 0) key_tap(*kfd, KEY_APPSELECT);
        }
    }
}

// Bind the abstract unix socket the laptop reaches via `adb forward`.
static int open_socket(void) {
    int srv = socket(AF_UNIX, SOCK_STREAM, 0);
    if (srv < 0) return -1;
    struct sockaddr_un addr;
    memset(&addr, 0, sizeof(addr));
    addr.sun_family = AF_UNIX;
    addr.sun_path[0] = '\0'; // abstract namespace
    strcpy(addr.sun_path + 1, SOCKET_NAME);
    socklen_t len = offsetof(struct sockaddr_un, sun_path) + 1 + strlen(SOCKET_NAME);
    if (bind(srv, (struct sockaddr *)&addr, len) < 0) {
        fprintf(stderr, "vortex_inject: bind failed: %s\n", strerror(errno));
        close(srv);
        return -1;
    }
    if (listen(srv, 1) < 0) {
        close(srv);
        return -1;
    }
    return srv;
}

int main(int argc, char **argv) {
    for (int i = 1; i < argc; i++) {
        if (strcmp(argv[i], "--keep-keys") == 0) keep_keys = 1;
        // Force the fallback even where uinput would work. The only way to
        // exercise the UHID path on a phone that does not need it — which is
        // every phone we can test on.
        if (strcmp(argv[i], "--uhid") == 0) backend_uhid = 1;
    }

    // Test mode: one tap, then exit (needs the device immediately).
    if (argc >= 4 && strcmp(argv[1], "tap") == 0) {
        int fd = setup_device();
        if (fd < 0) return 1;
        touch_down(fd, 0, atoi(argv[2]), atoi(argv[3]));
        usleep(60 * 1000);
        touch_up(fd, 0);
        usleep(50 * 1000);
        ioctl(fd, UI_DEV_DESTROY);
        close(fd);
        return 0;
    }

    // Mouse test: create the relative pointer and wiggle it in a visible square
    // for ~4s so you can SEE the phone render + move its native cursor (the
    // Universal Control proof). No socket.
    if (argc >= 2 && strcmp(argv[1], "mouse-test") == 0) {
        if (access("/dev/uinput", W_OK) != 0) backend_uhid = 1;
        int mfd = setup_mouse_device(700);
        if (mfd < 0) return 1;
        // Nudge once so Android materializes the cursor, then trace a square.
        mouse_move(mfd, 1, 1);
        usleep(300 * 1000);
        const int steps = 40, step_px = 12;
        for (int loop = 0; loop < 3; loop++) {
            for (int i = 0; i < steps; i++) { mouse_move(mfd, step_px, 0); usleep(12 * 1000); }
            for (int i = 0; i < steps; i++) { mouse_move(mfd, 0, step_px); usleep(12 * 1000); }
            for (int i = 0; i < steps; i++) { mouse_move(mfd, -step_px, 0); usleep(12 * 1000); }
            for (int i = 0; i < steps; i++) { mouse_move(mfd, 0, -step_px); usleep(12 * 1000); }
        }
        usleep(200 * 1000);
        destroy_device(mfd);
        return 0;
    }

    // Hold the keyboard up so Android's one-time "Configure physical keyboard"
    // can actually be answered.
    //
    // That screen configures a specific device, and closes the moment the device
    // goes away — which is exactly what happens when you pick the phone up to
    // tap the notification, because control returns to the laptop and the
    // keyboard is destroyed with it. So there was no window in which the layout
    // could be chosen at all. This mode keeps the SAME device identity alive on
    // its own, and Android files the layout under the device descriptor, so it
    // still applies to the real one afterwards.
    if (argc >= 2 && strcmp(argv[1], "keys-hold") == 0) {
        int secs = argc >= 3 ? atoi(argv[2]) : 120;
        if (secs < 1) secs = 1;
        if (access("/dev/uinput", W_OK) != 0) backend_uhid = 1;
        int kfd = setup_key_device();
        if (kfd < 0) return 1;
        fprintf(stderr, "vortex_inject: keyboard held for %ds — pick a layout now\n", secs);
        sleep(secs);
        destroy_device(kfd);
        return 0;
    }

    // CRITICAL: bind + listen BEFORE the (~700ms) uinput setup. Otherwise the
    // laptop, connecting through `adb forward` during that window, races ahead
    // of us and gets a dead tunnel — its first write fails and all input is
    // silently lost. Listening first lets the connection queue in the backlog
    // until we accept it.
    int srv = open_socket();
    if (srv < 0) return 1;
    fprintf(stderr, "vortex_inject: listening on @%s\n", SOCKET_NAME);

    // The touchscreen is the canary: it is the one device only uinput can make,
    // so failing to open it is how we learn this phone does not hand shell
    // /dev/uinput at all. That is the common case off MIUI, and it is not fatal
    // — the cursor and the keyboard can still go through UHID. Only the mirror's
    // taps and the drawn-finger scroll are lost, and both degrade to something.
    int fd = backend_uhid ? -1 : setup_device();
    if (fd < 0 && !backend_uhid) {
        backend_uhid = 1;
        fprintf(stderr, "vortex_inject: no /dev/uinput for shell — UHID backend "
                        "(cursor + keyboard; no touch injection)\n");
    }
    // The mouse is created lazily, on the first `V 1`: one that exists from
    // startup would put an idle pointer on the phone's screen before the user
    // has crossed over. Until then mfd = -1 → P/B/W are no-ops, and the mirror's
    // touch path is unaffected.
    int mfd = -1;
    // The keyboard has two lifetimes, and the laptop picks which one.
    //
    // Cycling it with each crossing costs a "Configure physical keyboard"
    // notification EVERY time: Android posts one whenever an alphabetic keyboard
    // appears with no layout saved for it, and no layout can be saved from here
    // (MIUI refuses shell both INJECT_EVENTS and WRITE_SECURE_SETTINGS, and the
    // picker closes the instant the device it is configuring disappears).
    //
    // Leaving it attached costs the phone its own on-screen keyboard, because
    // Android hides that while a hardware one is present — UNLESS
    // `show_ime_with_hard_keyboard` is on, which is off by default.
    //
    // So neither is right in general, and the laptop reads that setting and
    // passes `--keep-keys` when it is safe. Without it we cycle, and take the
    // notification.
    int kfd = keep_keys ? setup_key_device() : -1;

    int cli = accept(srv, NULL, NULL);
    if (cli < 0) {
        close(srv);
        destroy_device(fd);
        destroy_device(mfd);
        destroy_device(kfd);
        return 1;
    }
    fprintf(stderr, "vortex_inject: client connected\n");

    char buf[512];
    int buflen = 0;
    int running = 1;
    while (running) {
        int n = read(cli, buf + buflen, sizeof(buf) - buflen - 1);
        if (n <= 0) break;
        buflen += n;
        buf[buflen] = '\0';
        char *start = buf;
        char *nl;
        while ((nl = strchr(start, '\n')) != NULL) {
            *nl = '\0';
            if (start[0] == 'Q') {
                running = 0;
                break;
            }
            process_line(fd, &mfd, &kfd, start);
            start = nl + 1;
        }
        int rem = buflen - (int)(start - buf);
        if (rem > 0) memmove(buf, start, rem);
        buflen = rem;
    }

    close(cli);
    close(srv);
    destroy_device(fd);
    destroy_device(mfd);
    destroy_device(kfd);
    return 0;
}
