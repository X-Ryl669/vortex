import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Mirrors Rust `core::call_log::CallLogEntry` / Kotlin `core.calllog.CallLogEntry`.
 *  `type` follows CallLog.Calls.TYPE: 1=incoming 2=outgoing 3=missed 4=voicemail
 *  5=rejected 6=blocked. `date` is epoch ms; `duration` is seconds. */
export interface CallLogEntry {
  id: string;
  number: string;
  name: string;
  type: number;
  date: number;
  duration: number;
}

export const callLog = ref<CallLogEntry[]>([]);
export const callLogLoaded = ref(false);

// Full call history synced over LAN (watermark dataset) — the recent-60
// list above stays the fast/BLE-fallback source; this fills the rest.
export const callHistory = ref<CallLogEntry[]>([]);

/** Recent list + LAN-synced history, deduped by id (recent wins — its
 *  cached-name resolution is fresher), newest first. The Recents page
 *  renders this. */
export const allCalls = computed<CallLogEntry[]>(() => {
  const byId = new Map<string, CallLogEntry>();
  for (const e of [...callHistory.value, ...callLog.value]) {
    byId.set(e.id || `${e.date}-${e.number}`, e);
  }
  return [...byId.values()].sort((a, b) => b.date - a.date);
});

let started = false;

/** Subscribe to the call-log mirror: seed from the daemon's disk cache, then
 *  live-update on every `vortex:call_log` push as the phone re-syncs. */
export async function initRecents(): Promise<void> {
  if (started) return;
  started = true;

  try {
    callLog.value = await invoke<CallLogEntry[]>("get_call_log");
    callLogLoaded.value = true;
  } catch {
    // No cache yet — the first BLE sync will fill it.
  }
  try {
    callHistory.value = await invoke<CallLogEntry[]>("get_call_log_history");
  } catch {
    // No history store yet — the first LAN bulk-sync backfills it.
  }

  await listen<CallLogEntry[]>("vortex:call_log", (e) => {
    callLog.value = e.payload ?? [];
    callLogLoaded.value = true;
  });

  // LAN bulk-sync merged a history batch → adopt the full updated store.
  await listen<CallLogEntry[]>("vortex:call-log-history", (e) => {
    callHistory.value = e.payload ?? [];
  });
}
