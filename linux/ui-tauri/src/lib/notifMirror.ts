import { ref } from "vue";
import {
  setNotifMirrorShow,
  getNotifMirrorShow,
  setNotifMirrorSend,
  getNotifMirrorSend,
} from "./bridge";

/**
 * Whether THIS laptop shows mirrored phone notifications. Per-device,
 * LOCAL-only — NOT synced with the phone (each side controls what it
 * displays). Persisted in localStorage; re-applied to the daemon on mount
 * (the daemon defaults to ON and resets on restart).
 */
const STORAGE_KEY = "vortex.notif_mirror_show";

export const notifMirrorShow = ref<boolean>(
  localStorage.getItem(STORAGE_KEY) !== "false",
);

/** User toggled it: persist + tell the daemon to start/stop showing. */
export function setNotifMirror(show: boolean): void {
  notifMirrorShow.value = show;
  localStorage.setItem(STORAGE_KEY, String(show));
  void setNotifMirrorShow(show);
}

/**
 * Whether THIS laptop SENDS its own desktop notifications to the phone
 * (laptop→phone). Per-device, LOCAL-only. Persisted in localStorage,
 * re-applied to the daemon on mount.
 */
const SEND_KEY = "vortex.notif_mirror_send";

export const notifMirrorSend = ref<boolean>(
  localStorage.getItem(SEND_KEY) !== "false",
);

/** User toggled SEND: persist + tell the daemon to start/stop forwarding. */
export function setNotifSend(send: boolean): void {
  notifMirrorSend.value = send;
  localStorage.setItem(SEND_KEY, String(send));
  void setNotifMirrorSend(send);
}

/** Sync the saved values to the daemon on mount (the daemon defaults ON and
 *  resets on restart, so push our saved prefs). Reconciles show + send. */
export function initNotifMirror(): void {
  getNotifMirrorShow()
    .then((v) => {
      if (notifMirrorShow.value !== v) void setNotifMirrorShow(notifMirrorShow.value);
    })
    .catch(() => {
      if (!notifMirrorShow.value) void setNotifMirrorShow(false);
    });
  getNotifMirrorSend()
    .then((v) => {
      if (notifMirrorSend.value !== v) void setNotifMirrorSend(notifMirrorSend.value);
    })
    .catch(() => {
      if (!notifMirrorSend.value) void setNotifMirrorSend(false);
    });
}
