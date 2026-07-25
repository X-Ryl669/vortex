import { ref } from "vue";
import { listen } from "@tauri-apps/api/event";
import { setSmartSwitchEnabled, getSmartSwitchEnabled } from "./bridge";

/**
 * Smart audio-follow on/off. A SHARED, cross-device setting: the daemon owns
 * the persisted value + LWW timestamp and syncs it with the phone over the
 * AppState heartbeat. This ref just mirrors the daemon for the Settings UI.
 *
 * Push happens only on an explicit user toggle ([`setSmartSwitch`]) — never
 * from a reactive watcher — so a value the daemon hands us back (initial
 * seed or a peer-driven LWW adoption) is NOT echoed back, which would
 * otherwise re-stamp the timestamp and ping-pong with the phone forever.
 */
export const smartSwitchEnabled = ref<boolean>(true);

/** User toggled it: update the UI and tell the daemon, which stamps the LWW
 *  timestamp, persists, and propagates to the phone. */
export function setSmartSwitch(v: boolean): void {
  smartSwitchEnabled.value = v;
  void setSmartSwitchEnabled(v);
}

/** Seed from the daemon and follow peer-driven (LWW) changes. Call once on
 *  app mount. */
export async function initSmartSwitch(): Promise<void> {
  try {
    smartSwitchEnabled.value = await getSmartSwitchEnabled();
  } catch {
    /* daemon not ready yet — stays at the default until the next event */
  }
  await listen<boolean>("vortex:smart_switch", (ev) => {
    smartSwitchEnabled.value = !!ev.payload;
  });
}
