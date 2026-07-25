import { createI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import en from "./locales/en.json";
import uz from "./locales/uz.json";
import ru from "./locales/ru.json";

export type LocaleCode = "en" | "uz" | "ru";

const STORAGE_KEY = "vortex.locale";
const STORAGE_KEY_TS = "vortex.locale_changed_at";

/**
 * Pick the initial locale. Priority:
 *   1. The user's explicit override stored in localStorage (carries a
 *      timestamp so cross-device LWW sync can adopt or override it).
 *   2. The system locale (navigator.language).
 *   3. "en".
 *
 * The `changedAt` is 0 for the "system default" case — it means
 * "I don't have an opinion, peer can override freely".
 */
function detect(): { code: LocaleCode; changedAt: number } {
  const stored = localStorage.getItem(STORAGE_KEY) as LocaleCode | null;
  const storedTs = parseInt(localStorage.getItem(STORAGE_KEY_TS) ?? "0", 10) || 0;
  if (stored && (stored === "en" || stored === "uz" || stored === "ru")) {
    return { code: stored, changedAt: storedTs };
  }
  const lang = (navigator.language || "en").toLowerCase();
  if (lang.startsWith("uz")) return { code: "uz", changedAt: 0 };
  if (lang.startsWith("ru")) return { code: "ru", changedAt: 0 };
  return { code: "en", changedAt: 0 };
}

const initial = detect();

/**
 * Mirror the chosen language to the standalone "Hey Vortex" voice assistant
 * (a separate Python process). It reads `~/.local/share/vortex/voice/lang` and
 * restarts itself when it changes, so the assistant follows the app's language.
 * Best-effort: ignored outside the Tauri runtime (e.g. in tests/dev web).
 */
function syncVoiceLang(loc: LocaleCode) {
  invoke("set_voice_lang", { code: loc }).catch(() => {});
}

export const i18n = createI18n({
  legacy: false,
  locale: initial.code,
  fallbackLocale: "en",
  messages: { en, uz, ru },
});

// Sync the voice assistant to the locale we started with (covers an existing
// localStorage choice that the assistant hasn't seen yet).
syncVoiceLang(initial.code);

/** Track whether the current locale was a user override (timestamp set). */
let currentChangedAt: number = initial.changedAt;

export function currentLocale(): LocaleCode {
  return i18n.global.locale.value as LocaleCode;
}

export function currentLocaleChangedAt(): number {
  return currentChangedAt;
}

/**
 * User-driven locale change (Settings → language pill). Records "now"
 * as the changedAt so any later cross-device sync uses LWW correctly.
 */
export function setLocale(loc: LocaleCode) {
  i18n.global.locale.value = loc;
  currentChangedAt = Math.floor(Date.now() / 1000);
  localStorage.setItem(STORAGE_KEY, loc);
  localStorage.setItem(STORAGE_KEY_TS, String(currentChangedAt));
  syncVoiceLang(loc);
}

/**
 * Peer-driven locale change (LWW: their `locale_changed_at` was newer
 * than ours). Adopts their locale and their timestamp — we don't bump
 * to "now" because we want subsequent local changes to win over this
 * adopted value.
 */
export function adoptPeerLocale(loc: LocaleCode, changedAt: number) {
  if (changedAt <= currentChangedAt) return;
  i18n.global.locale.value = loc;
  currentChangedAt = changedAt;
  localStorage.setItem(STORAGE_KEY, loc);
  localStorage.setItem(STORAGE_KEY_TS, String(changedAt));
  syncVoiceLang(loc);
}

export const LOCALES: { code: LocaleCode; label: string }[] = [
  { code: "uz", label: "O'zbekcha" },
  { code: "en", label: "English" },
  { code: "ru", label: "Русский" },
];
