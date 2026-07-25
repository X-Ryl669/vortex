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
// Protocol (newline-delimited):
//   D <slot> <nx> <ny>   finger down (slot = pointer index, for multitouch)
//   M <slot> <nx> <ny>   finger move
//   U <slot>             finger up
//   E <keycode> <val>    raw key event (val 1=down 0=up) — keyboard
//   K <back|home|recents> navigation key
//   Q                    quit
// Test mode (no socket): `vortex_inject tap <nx> <ny>`.

#include <errno.h>
#include <fcntl.h>
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

static int active_count = 0;
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

static int setup_device(void) {
    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) {
        fprintf(stderr, "vortex_inject: open /dev/uinput failed: %s\n", strerror(errno));
        return -1;
    }

    ioctl(fd, UI_SET_EVBIT, EV_SYN);
    ioctl(fd, UI_SET_EVBIT, EV_KEY);
    ioctl(fd, UI_SET_KEYBIT, BTN_TOUCH);
    // Enable the whole standard key range so the laptop can send any keystroke
    // (letters, digits, punctuation, enter/backspace/arrows, etc.).
    for (int code = 1; code <= 255; code++) ioctl(fd, UI_SET_KEYBIT, code);
    ioctl(fd, UI_SET_KEYBIT, KEY_BACK);
    ioctl(fd, UI_SET_KEYBIT, KEY_HOMEPAGE);
    ioctl(fd, UI_SET_KEYBIT, KEY_APPSELECT);

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

static void touch_down(int fd, int slot, int nx, int ny) {
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
    emit(fd, EV_ABS, ABS_MT_SLOT, slot);
    emit(fd, EV_ABS, ABS_MT_POSITION_X, nx);
    emit(fd, EV_ABS, ABS_MT_POSITION_Y, ny);
    emit(fd, EV_ABS, ABS_X, nx);
    emit(fd, EV_ABS, ABS_Y, ny);
    syn(fd);
}

static void touch_up(int fd, int slot) {
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
    emit(fd, EV_KEY, code, val);
    syn(fd);
}

static void key_tap(int fd, int code) {
    key_event(fd, code, 1);
    key_event(fd, code, 0);
}

static void process_line(int fd, char *line) {
    char c = line[0];
    int a, b, d;
    if (c == 'D' && sscanf(line + 1, "%d %d %d", &a, &b, &d) == 3) {
        touch_down(fd, a, b, d);
    } else if (c == 'M' && sscanf(line + 1, "%d %d %d", &a, &b, &d) == 3) {
        touch_move(fd, a, b, d);
    } else if (c == 'U' && sscanf(line + 1, "%d", &a) == 1) {
        touch_up(fd, a);
    } else if (c == 'E' && sscanf(line + 1, "%d %d", &a, &b) == 2) {
        key_event(fd, a, b);
    } else if (c == 'K') {
        char what[16];
        if (sscanf(line + 1, "%15s", what) == 1) {
            if (strcmp(what, "back") == 0) key_tap(fd, KEY_BACK);
            else if (strcmp(what, "home") == 0) key_tap(fd, KEY_HOMEPAGE);
            else if (strcmp(what, "recents") == 0) key_tap(fd, KEY_APPSELECT);
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

    // CRITICAL: bind + listen BEFORE the (~700ms) uinput setup. Otherwise the
    // laptop, connecting through `adb forward` during that window, races ahead
    // of us and gets a dead tunnel — its first write fails and all input is
    // silently lost. Listening first lets the connection queue in the backlog
    // until we accept it.
    int srv = open_socket();
    if (srv < 0) return 1;
    fprintf(stderr, "vortex_inject: listening on @%s\n", SOCKET_NAME);

    int fd = setup_device();
    if (fd < 0) {
        close(srv);
        return 1;
    }

    int cli = accept(srv, NULL, NULL);
    if (cli < 0) {
        close(srv);
        ioctl(fd, UI_DEV_DESTROY);
        close(fd);
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
            process_line(fd, start);
            start = nl + 1;
        }
        int rem = buflen - (int)(start - buf);
        if (rem > 0) memmove(buf, start, rem);
        buflen = rem;
    }

    close(cli);
    close(srv);
    ioctl(fd, UI_DEV_DESTROY);
    close(fd);
    return 0;
}
