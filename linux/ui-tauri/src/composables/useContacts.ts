import { ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Mirrors Rust `core::contacts::Contact` / Kotlin `core.contacts.Contact`. */
export interface Contact {
  id: string;
  name: string;
  numbers: string[];
}

// App-lifetime singletons (initialised once in main.ts, survive route changes).
export const contacts = ref<Contact[]>([]);
export const contactsLoaded = ref(false);

let started = false;

/**
 * Subscribe to the contacts mirror: seed from the daemon's disk cache
 * (instant on open / after a restart), then live-update on every `vortex:contacts`
 * push as the phone re-syncs (initial connect, contact added/edited/removed).
 */
export async function initContacts(): Promise<void> {
  if (started) return;
  started = true;

  try {
    contacts.value = await invoke<Contact[]>("get_contacts");
    contactsLoaded.value = true;
  } catch {
    // No cache yet — the first BLE sync will fill it.
  }

  await listen<Contact[]>("vortex:contacts", (e) => {
    contacts.value = e.payload ?? [];
    contactsLoaded.value = true;
  });
}
