import { invoke } from "@tauri-apps/api/core";

/**
 * Ask the phone to place an outgoing call to `number` (laptop-initiated dial,
 * from the Contacts or Recents page). The phone dials it and the normal in-call
 * pill appears. Best-effort — a failure just logs.
 */
export async function dial(number: string): Promise<void> {
  const n = (number ?? "").trim();
  if (!n) return;
  try {
    await invoke("dial", { number: n });
  } catch (e) {
    console.warn("dial failed", e);
  }
}
