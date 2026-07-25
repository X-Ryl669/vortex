import { ref } from "vue";

// First-run gate for the onboarding / intro flow. Same localStorage idiom as
// theme.ts / notifMirror.ts: a single boolean key, mirrored into a reactive ref
// so App.vue can switch the root between the intro and the shell.
const STORAGE_KEY = "vortex.intro_done";

export const introDone = ref(localStorage.getItem(STORAGE_KEY) === "true");

/** Mark the intro as completed (or skipped) — never shown again on launch. */
export function markIntroDone(): void {
  introDone.value = true;
  localStorage.setItem(STORAGE_KEY, "true");
}
