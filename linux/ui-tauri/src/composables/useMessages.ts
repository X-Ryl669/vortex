import { computed, ref, reactive } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Mirrors Rust `core::sms::SmsMessage` / Kotlin `core.sms.SmsMessage`.
 *  `type`: 1=inbox (received), 2=sent. `date` epoch ms; `thread` groups a
 *  conversation; `read`: 0=unread, 1=read. `status` is set only on optimistic
 *  laptop-sent entries that haven't been confirmed by a sync yet. */
export interface SmsMessage {
  id: string;
  address: string;
  body: string;
  type: number;
  date: number;
  thread: number;
  read: number;
  status?: "sending" | "failed";
  /** Failed before ever reaching the phone (no transport) — safe to
   *  auto-retry once when connectivity returns; an unconfirmed-echo
   *  failure is NOT (the SMS may actually have gone out). */
  hardFail?: boolean;
  /** One automatic retry already spent. */
  autoRetried?: boolean;
}

export const sms = ref<SmsMessage[]>([]);
export const smsLoaded = ref(false);
// Optimistically-shown messages the laptop just sent (sending → confirmed by a
// sync, or → failed after a timeout). Confirmed ones are pruned when the real
// message arrives (matched by number + body).
export const localSent = ref<SmsMessage[]>([]);

// On-demand thread history (infinite scroll), keyed by convKey(address). The
// recent-SMS sync (`sms`) only carries the latest ~80 across ALL threads; when
// a conversation is opened we ask the phone for its full history a page at a
// time (older as the user scrolls up) and merge it here. Merged by id, so it
// overlaps the recent list harmlessly. Mirrors the phone's `load_thread`.
export const threadPages = ref<Record<string, SmsMessage[]>>({});

/** How many messages a thread page requests (and the BLE-reliable ceiling for
 *  one on-demand burst — small, so it never desyncs the cipher). */
const PAGE = 40;

interface Cursor {
  offset: number; // messages already loaded for this thread
  limit: number; // size of the in-flight request
  done: boolean; // phone returned a short page → no older messages left
  loading: boolean;
}
// Reactive so the "loading older" spinner / "has more" state in the UI updates
// when a page lands (a plain Map wouldn't trigger recomputation, leaving the
// spinner stuck after the last page).
const cursors = reactive(new Map<string, Cursor>());
// The digits of the thread whose page request is in flight — lets us mark an
// EMPTY page (which carries no address) as "done" for the right thread.
let lastRequested: string | null = null;

/** Stable conversation key. A phone number collapses to its last 9 digits (so
 *  +998.. and 0.. forms of the same number match); an alphanumeric sender (bank
 *  / OTP / promo — no digits) falls back to its trimmed address. WITHOUT the
 *  fallback every digit-less sender keyed to "" and collided — opening one
 *  service SMS marked them ALL read. Must match the page's grouping exactly. */
export const convKey = (s: string) => (s || "").replace(/\D/g, "").slice(-9) || (s || "").trim();

// Full SMS history synced over LAN (watermark dataset): every message the
// phone has ever shipped, persisted on disk laptop-side. Conversations and
// threads merge it with the recent list — old threads render instantly with
// no per-thread network round-trip when this is populated. The BLE
// infinite-scroll path stays as the LAN-less fallback (id-dedup makes its
// overlap harmless).
export const history = ref<SmsMessage[]>([]);

// (The SMS/call app-id allowlist now lives in the backend — `notifications.rs`
// `notif_click_target` — as the single source of truth: it gates BOTH the
// window focus and which `kind` it emits, so this layer just routes by `kind`.)

let started = false;

/** Subscribe to the SMS mirror: seed from the daemon's disk cache, then
 *  live-update on every `vortex:sms` push as the phone re-syncs. */
export async function initMessages(): Promise<void> {
  if (started) return;
  started = true;

  try {
    sms.value = await invoke<SmsMessage[]>("get_sms");
    smsLoaded.value = true;
  } catch {
    // No cache yet — the first BLE sync will fill it.
  }
  try {
    history.value = await invoke<SmsMessage[]>("get_sms_history");
  } catch {
    // No history store yet — the first LAN bulk-sync backfills it.
  }

  await listen<SmsMessage[]>("vortex:sms", (e) => {
    sms.value = e.payload ?? [];
    smsLoaded.value = true;
    reconcilePending();
  });

  // LAN bulk-sync merged a history batch → adopt the full updated store.
  // A just-sent message may confirm through here first (history watermark
  // beats the recent-list resend), so reconcile optimistic sends too.
  await listen<SmsMessage[]>("vortex:sms-history", (e) => {
    history.value = e.payload ?? [];
    reconcilePending();
  });

  // A page of one conversation's history arrived → merge it into that thread.
  await listen<SmsMessage[]>("vortex:sms-thread", (e) => {
    mergeThreadPage(e.payload ?? []);
  });

  // Connectivity returned (any peer-state push means a live link): give
  // transport-failed sends ONE automatic retry. Unconfirmed-echo failures
  // are excluded — the SMS may actually have gone out, and auto-resending
  // those could double-text the recipient.
  await listen("vortex:peer_state", () => {
    for (const m of localSent.value) {
      if (m.status === "failed" && m.hardFail && !m.autoRetried) {
        m.autoRetried = true;
        void resendSms(m.id);
      }
    }
  });

  // The user clicked a mirrored notification's body: the backend focused the
  // window and handed us its title (usually the sender's display name).
  // Resolve it to a number — contact name match first, then a conversation
  // whose messages came from a matching named sender isn't resolvable from
  // here, so fall back to the title itself when it already IS an address
  // (number or alphanumeric sender ID present in our threads). No match →
  // just stay focused wherever we are.
  await listen<{ title: string; appId: string; kind?: string }>("vortex:open-chat", async (e) => {
    // The backend already gated which app types open here (SMS / call); it
    // tells us WHICH via `kind` so we route to the right page. Everything else
    // never reaches this handler (it just dismisses on the laptop + phone).
    const kind = e.payload?.kind ?? "";
    const { router } = await import("@/router");
    if (kind === "call") {
      // Missed-call / voicemail notification → open the call log.
      void router.push("/recents");
      return;
    }
    if (kind !== "sms") return; // defensive: backend only emits sms/call
    const title = (e.payload?.title ?? "").trim();
    if (!title) return;
    const { contacts } = await import("@/composables/useContacts");
    const lower = title.toLowerCase();
    const byContact = contacts.value.find((c) => c.name.trim().toLowerCase() === lower);
    const known = (addr: string) =>
      [...sms.value, ...history.value].some((m) => convKey(m.address) === convKey(addr));
    let target: string | null = null;
    if (byContact?.numbers[0]) {
      target = byContact.numbers[0];
    } else if (known(title)) {
      // The title is itself an address we have threads for (alphanumeric
      // sender IDs like banks arrive exactly this way).
      target = title;
    }
    if (target) {
      void router.push(`/messages/${encodeURIComponent(target)}`);
    }
  });

  // Voice assistant: `vortex-ui-tauri --sms <number>` (and `--sms-send`) emit
  // this with an already-resolved phone number → open that conversation thread
  // so the dictated message is composed/sent in plain view.
  await listen<{ number: string }>("vortex:open-sms", async (e) => {
    const number = (e.payload?.number ?? "").trim();
    if (!number) return;
    const { router } = await import("@/router");
    void router.push(`/messages/${encodeURIComponent(number)}`);
  });
}

/** Merge an on-demand thread page into [threadPages] (dedup by id) and advance
 *  that thread's pagination cursor. An empty page means the thread is fully
 *  loaded (no older messages). */
function mergeThreadPage(messages: SmsMessage[]): void {
  if (messages.length === 0) {
    if (lastRequested) {
      const cur = cursors.get(lastRequested);
      if (cur) {
        cur.done = true;
        cur.loading = false;
      }
    }
    return;
  }
  const d = convKey(messages[0].address);
  const existing = threadPages.value[d] ?? [];
  const byId = new Map(existing.map((m) => [m.id, m]));
  for (const m of messages) byId.set(m.id, m);
  threadPages.value = { ...threadPages.value, [d]: [...byId.values()] };
  const cur = cursors.get(d);
  if (cur) {
    cur.offset += messages.length;
    cur.loading = false;
    if (messages.length < cur.limit) cur.done = true;
  }
}

/** A page request the phone never answers (SMS mirror toggled off, BLE down)
 *  must not wedge the thread: after this deadline the spinner clears and the
 *  cursor unwinds so the page can be re-requested. */
const PAGE_TIMEOUT_MS = 12_000;

/** Arm a recovery timer for the page request just sent for thread [d]. If the
 *  reply lands first, `loading` is already false (mergeThreadPage) and this is
 *  a no-op. An unanswered page 0 forgets the cursor entirely so reopening the
 *  thread asks again; a later page just unblocks scroll-up retries. */
function armPageTimeout(d: string): void {
  const offsetAtSend = cursors.get(d)?.offset ?? 0;
  window.setTimeout(() => {
    const cur = cursors.get(d);
    if (!cur || !cur.loading || cur.offset !== offsetAtSend) return;
    cur.loading = false;
    if (cur.offset === 0) cursors.delete(d);
  }, PAGE_TIMEOUT_MS);
}

/** Request the first page of a conversation's history when it's opened. Cheap to
 *  call repeatedly — it no-ops once a thread already has a cursor. */
export async function loadThread(address: string, thread: number): Promise<void> {
  const d = convKey(address);
  if (cursors.has(d)) return; // already loading / loaded page 0
  cursors.set(d, { offset: 0, limit: PAGE, done: false, loading: true });
  lastRequested = d;
  try {
    await invoke("load_sms_thread", { thread: thread || 0, number: address, offset: 0, limit: PAGE });
    armPageTimeout(d);
  } catch (e) {
    console.warn("load_sms_thread failed", e);
    cursors.delete(d); // let the next open retry page 0
  }
}

/** Load the next (older) page of a conversation as the user scrolls up. No-ops
 *  when a page is in flight or the thread is fully loaded. */
export async function loadMoreThread(address: string, thread: number): Promise<void> {
  const d = convKey(address);
  const cur = cursors.get(d);
  if (!cur) return loadThread(address, thread); // page-0 timeout unwound it — start over
  if (cur.loading || cur.done) return;
  cur.loading = true;
  cur.limit = PAGE;
  lastRequested = d;
  try {
    await invoke("load_sms_thread", { thread: thread || 0, number: address, offset: cur.offset, limit: PAGE });
    armPageTimeout(d);
  } catch (e) {
    console.warn("load_sms_thread (more) failed", e);
    cur.loading = false;
  }
}

/** Whether a thread has more (older) messages to load — drives the scroll
 *  sentinel. Reactive (cursors is a reactive Map). */
export function threadHasMore(address: string): boolean {
  const cur = cursors.get(convKey(address));
  return !cur || !cur.done;
}

/** True when the LAN-synced full history already holds this thread's
 *  messages — the BLE page-fetch path is then pure overhead (every page
 *  would come back as duplicates) and the thread view reads straight from
 *  local data. */
export function threadCoveredByHistory(address: string): boolean {
  const d = convKey(address);
  return history.value.some((m) => convKey(m.address) === d);
}

/** Whether a thread page is currently being fetched — drives the spinner, so it
 *  shows only during an actual load (not as a permanent "more exists" hint). */
export function threadLoading(address: string): boolean {
  return cursors.get(convKey(address))?.loading ?? false;
}

/** Drop optimistic entries the real sync now contains (same number + body) —
 *  checked against BOTH the recent list and the LAN-synced history, since
 *  either may carry the confirmation first. */
function reconcilePending(): void {
  if (!localSent.value.length) return;
  const confirmed = (l: SmsMessage) =>
    [...sms.value, ...history.value].some(
      (m) => m.type === 2 && convKey(m.address) === convKey(l.address) && m.body === l.body,
    );
  localSent.value = localSent.value.filter((l) => !confirmed(l));
}

// Deliver the command to the phone. Success (it reached the phone over BLE, or
// was queued for the LAN fallback) → clear the spinner: the message reads as
// sent. Only a hard invoke error (no transport at all) → "failed". We don't
// wait for a sync round-trip to confirm — that path is flaky and would leave a
// stuck spinner on a message that actually went out.
async function fire(id: string, to: string, body: string): Promise<void> {
  try {
    await invoke("send_sms", { number: to, body });
    const e = localSent.value.find((x) => x.id === id);
    if (e) e.status = undefined;
  } catch (err) {
    console.warn("send_sms failed", err);
    const e = localSent.value.find((x) => x.id === id);
    if (e) {
      e.status = "failed";
      e.hardFail = true; // never reached the phone — auto-retry is safe
    }
  }
}

/** Send an SMS from the laptop. Shows a brief "sending" spinner, then the
 *  message reads as sent once the command reaches the phone. */
export async function sendSms(to: string, body: string): Promise<void> {
  const t = (to ?? "").trim();
  const b = (body ?? "").trim();
  if (!t || !b) return;
  const id = `local-${Date.now()}-${Math.floor(Math.random() * 1e6)}`;
  localSent.value = [
    ...localSent.value,
    { id, address: t, body: b, type: 2, date: Date.now(), thread: 0, read: 1, status: "sending" },
  ];
  await fire(id, t, b);
  armConfirmTimeout(id);
}

/** End-to-end send confirmation: the command reaching the phone proves
 *  nothing about the SMS actually going out (no signal, SIM error). The real
 *  proof is the message echoing back through the phone's own SMS table via
 *  the recent/history sync — reconcilePending then removes the optimistic
 *  entry. If that echo doesn't arrive within the deadline, surface it as
 *  failed so the user gets the resend/delete affordances instead of a
 *  false "sent". Deadline sized over the slowest legit path (BLE down →
 *  LAN-queued command + next heartbeat sync ≈ 25s). */
const SEND_CONFIRM_TIMEOUT_MS = 60_000;
function armConfirmTimeout(id: string): void {
  window.setTimeout(() => {
    const e = localSent.value.find((x) => x.id === id);
    if (e && e.status === undefined) {
      console.warn("send not confirmed by sync; marking failed");
      e.status = "failed";
    }
  }, SEND_CONFIRM_TIMEOUT_MS);
}

/** Retry a failed message (same body, fresh attempt). */
export async function resendSms(id: string): Promise<void> {
  const entry = localSent.value.find((x) => x.id === id);
  if (!entry) return;
  entry.status = "sending";
  entry.date = Date.now();
  await fire(id, entry.address, entry.body);
  armConfirmTimeout(id);
}

/** Discard a failed (or any) optimistic message. */
export function deleteLocalSent(id: string): void {
  localSent.value = localSent.value.filter((x) => x.id !== id);
}

// Laptop-side read-state: a per-conversation "read up to" timestamp, persisted
// so it survives re-syncs (each `vortex:sms` REPLACES the list, wiping any
// in-place read flag). We can't write the phone's SMS provider — only the
// default SMS app may, a platform rule — so this is the laptop's own view, just
// like other phone-companion apps. Reading the thread ON THE PHONE clears it too
// (the phone's own read=1 wins regardless of the watermark).
const READ_KEY = "vortex.sms.readWatermark";
function loadWatermark(): Record<string, number> {
  try {
    return JSON.parse(localStorage.getItem(READ_KEY) || "{}");
  } catch {
    return {};
  }
}
export const readWatermark = ref<Record<string, number>>(loadWatermark());

/** A received message counts as read on the laptop if the phone already marked
 *  it read OR the laptop has opened that conversation since it arrived. */
export function isMessageRead(m: SmsMessage): boolean {
  if (m.read !== 0) return true;
  return m.date <= (readWatermark.value[convKey(m.address)] ?? 0);
}

/** Number of conversations whose newest RECEIVED message is unread — the
 *  sidebar badge. Same last-received logic the conversation list uses. */
export const unreadConversations = computed(() => {
  const latestIn = new Map<string, SmsMessage>();
  for (const m of [...sms.value, ...history.value]) {
    if (m.type !== 1) continue;
    const k = convKey(m.address);
    const cur = latestIn.get(k);
    if (!cur || m.date > cur.date) latestIn.set(k, m);
  }
  let n = 0;
  for (const m of latestIn.values()) if (!isMessageRead(m)) n++;
  return n;
});

// Per-conversation drafts (Telegram-style): leaving a chat keeps what you
// were typing; persisted so an app restart keeps them too.
const DRAFTS_KEY = "vortex.sms.drafts";
function loadDrafts(): Record<string, string> {
  try {
    return JSON.parse(localStorage.getItem(DRAFTS_KEY) || "{}");
  } catch {
    return {};
  }
}
const draftsStore = ref<Record<string, string>>(loadDrafts());
export function getDraft(address: string): string {
  return draftsStore.value[convKey(address)] ?? "";
}
export function setDraft(address: string, text: string): void {
  const k = convKey(address);
  const next = { ...draftsStore.value };
  if (text) next[k] = text;
  else delete next[k];
  draftsStore.value = next;
  try {
    localStorage.setItem(DRAFTS_KEY, JSON.stringify(next));
  } catch {
    /* private mode / quota */
  }
}

/** The laptop opened a conversation → remember everything up to now is read, so
 *  it stays un-bold across re-syncs. Laptop-only (the phone can't be written). */
export function markConversationRead(address: string): void {
  const k = convKey(address);
  readWatermark.value = { ...readWatermark.value, [k]: Date.now() };
  try {
    localStorage.setItem(READ_KEY, JSON.stringify(readWatermark.value));
  } catch {
    /* private mode / quota — read-state just won't persist this session */
  }
}
