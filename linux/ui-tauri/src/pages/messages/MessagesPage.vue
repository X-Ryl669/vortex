<script setup lang="ts">
import { computed, ref, watch, nextTick, onMounted, onUnmounted } from "vue";
import { useI18n } from "vue-i18n";
import { useRoute, useRouter } from "vue-router";
import { useVirtualList } from "@vueuse/core";
import { MessageSquare, ArrowLeft, ArrowDown, Send, Loader2, AlertCircle, RotateCw, Trash2, Smile, Phone } from "lucide-vue-next";
import SearchInput from "@/components/SearchInput.vue";
import Avatar from "@/components/Avatar.vue";
import ConversationRow from "./ConversationRow.vue";
import { dial } from "@/lib/dial";
import { invoke } from "@tauri-apps/api/core";
// Emoji picker web component — registered as <emoji-picker> on import. The
// emoji data ships as a LOCAL vite asset (the library's CDN default would be
// blocked by the Tauri CSP).
import "emoji-picker-element";
import emojiDataUrl from "emoji-picker-element-data/en/emojibase/data.json?url";
import { theme } from "@/lib/theme";
import {
  sms,
  smsLoaded,
  localSent,
  history,
  threadPages,
  sendSms,
  resendSms,
  deleteLocalSent,
  markConversationRead,
  isMessageRead,
  loadThread,
  loadMoreThread,
  threadHasMore,
  threadCoveredByHistory,
  threadLoading,
  getDraft,
  setDraft,
  convKey,
  type SmsMessage,
} from "@/composables/useMessages";
import { contacts } from "@/composables/useContacts";

const { t } = useI18n();
const route = useRoute();
const router = useRouter();

const digits = (s: string) => s.replace(/\D/g, "").slice(-9);

// Resolve a number to a contact name via the synced contacts (suffix-match).
const nameByDigits = computed(() => {
  const map = new Map<string, string>();
  for (const c of contacts.value) {
    for (const n of c.numbers) {
      const d = digits(n);
      if (d.length >= 7) map.set(d, c.name);
    }
  }
  return map;
});
function displayName(address: string): string {
  return nameByDigits.value.get(digits(address)) || address || t("messages.unknown");
}

interface Conversation {
  address: string;
  name: string;
  last: SmsMessage;
  count: number;
  unread: boolean;
  /** Unread received messages since the last READ one (TG-style badge). */
  unreadCount: number;
}

// All messages = synced + optimistic local sent.
// Recent sync + LAN-synced full history + optimistic local sends. The
// conversation grouping below dedups by id, so the overlap between recent
// and history is harmless; history makes OLD threads appear in the list.
const allMessages = computed(() => [...sms.value, ...history.value, ...localSent.value]);

// Conversation list grouped by number (newest first).
const conversations = computed<Conversation[]>(() => {
  const byNum = new Map<string, SmsMessage[]>();
  // Dedup by id across the sources (recent + history overlap); first copy
  // wins, and allMessages lists the recent sync first — its read-flags are
  // fresher than the history store's sync-time snapshot.
  const seen = new Set<string>();
  for (const m of allMessages.value) {
    const id = m.id || `${m.date}-${m.body}`;
    if (seen.has(id)) continue;
    seen.add(id);
    const k = convKey(m.address);
    (byNum.get(k) ?? byNum.set(k, []).get(k)!).push(m);
  }
  const list: Conversation[] = [];
  for (const msgs of byNum.values()) {
    msgs.sort((a, b) => a.date - b.date);
    const last = msgs[msgs.length - 1];
    // Unread = the newest *received* message hasn't been read (matches how a
    // phone bolds a thread). Per-message read=0 alone is too noisy — old buried
    // messages can stay read=0 — so we only look at the latest incoming one.
    const lastReceived = [...msgs].reverse().find((m) => m.type === 1);
    const unread = !!lastReceived && !isMessageRead(lastReceived);
    // Badge count: received messages NEWER than the last read one. Bounding
    // it that way keeps old buried read=0 rows (provider noise) out.
    let unreadCount = 0;
    if (unread) {
      const lastReadAt = [...msgs]
        .reverse()
        .find((m) => m.type === 1 && isMessageRead(m))?.date ?? 0;
      unreadCount = msgs.filter(
        (m) => m.type === 1 && m.date > lastReadAt && !isMessageRead(m),
      ).length;
    }
    list.push({ address: last.address, name: displayName(last.address), last, count: msgs.length, unread, unreadCount });
  }
  list.sort((a, b) => b.last.date - a.last.date);
  return list;
});

// The open conversation, addressed by number (works for an existing thread or a
// brand-new compose target passed from Contacts via ?to=).
// The open thread is ROUTE state (/messages/:address), not local state —
// history (and the mouse back button) then closes the chat naturally.
const activeNumber = computed<string | null>(() => {
  const p = route.params.address;
  return typeof p === "string" && p ? decodeURIComponent(p) : null;
});
const draft = ref("");
const threadEl = ref<HTMLElement | null>(null);
const composerEl = ref<HTMLTextAreaElement | null>(null);

// Emoji picker state. Emoji are plain Unicode so they ride SMS as-is —
// the closest thing to "stickers" the SMS protocol allows.
// Telegram-Desktop behaviour: hovering the smiley opens the panel, it stays
// while the pointer is over the button OR the panel (a short grace bridges
// the gap between them), and leaves close it. Click toggles too (touch),
// and any click outside the zone closes it.
const emojiOpen = ref(false);
let emojiCloseTimer: number | null = null;
function emojiEnter() {
  if (emojiCloseTimer !== null) {
    clearTimeout(emojiCloseTimer);
    emojiCloseTimer = null;
  }
  emojiOpen.value = true;
}
function emojiLeave() {
  if (emojiCloseTimer !== null) clearTimeout(emojiCloseTimer);
  emojiCloseTimer = window.setTimeout(() => {
    emojiOpen.value = false;
    emojiCloseTimer = null;
  }, 250);
}
function onDocClick(e: MouseEvent) {
  if (!emojiOpen.value) return;
  if (!(e.target as HTMLElement).closest?.(".emoji-zone")) emojiOpen.value = false;
}
onMounted(() => document.addEventListener("click", onDocClick, true));
onUnmounted(() => document.removeEventListener("click", onDocClick, true));

// Telegram-style growing composer: the textarea stretches with the draft
// up to its max height, then scrolls; clearing the draft shrinks it back.
// scrollHeight excludes the border while style.height (border-box)
// includes it — without adding the border back the first keystroke
// visibly shrank the field by 2px.
function autoGrow() {
  const el = composerEl.value;
  if (!el) return;
  const border = el.offsetHeight - el.clientHeight;
  el.style.height = "auto";
  el.style.height = `${Math.min(el.scrollHeight + border, 112)}px`;
}
watch(draft, () => void nextTick(autoGrow));

// No-reply senders: an alphanumeric sender ID (banks, carriers, OTP
// services — any letters in the address) cannot receive SMS per the GSM
// spec, so the composer is replaced with a note. Numeric addresses
// (including short codes) keep the composer — some short codes do accept
// replies, and blocking them would be a guess.
const canReply = computed(() => {
  const a = activeNumber.value ?? "";
  return !!a && !/[a-zA-Z]/.test(a);
});

const isDark = computed(() => theme.value === "dark");
function onEmoji(e: Event) {
  const unicode = (e as CustomEvent).detail?.unicode as string | undefined;
  if (!unicode) return;
  const el = composerEl.value;
  if (el && typeof el.selectionStart === "number") {
    const pos = el.selectionStart;
    draft.value = draft.value.slice(0, pos) + unicode + draft.value.slice(el.selectionEnd);
    void nextTick(() => {
      el.selectionStart = el.selectionEnd = pos + unicode.length;
    });
  } else {
    draft.value += unicode;
  }
}

const activeName = computed(() => (activeNumber.value ? displayName(activeNumber.value) : ""));
const activeMessages = computed(() => {
  if (!activeNumber.value) return [];
  const d = convKey(activeNumber.value);
  // Merge the LAN-synced history, the recent-sync list, the on-demand BLE
  // pages and optimistic local sends — deduped by id (later sources
  // overwrite, so fresher read-flags from the recent sync win over the
  // history store's sync-time snapshot).
  const byId = new Map<string, SmsMessage>();
  const pages = threadPages.value[d] ?? [];
  for (const m of [...history.value, ...sms.value, ...pages, ...localSent.value]) {
    if (convKey(m.address) === d) byId.set(m.id || `${m.date}-${m.body}`, m);
  }
  return [...byId.values()].sort((a, b) => a.date - b.date);
});
// Conversation search (same SearchInput as Contacts/Recents): matches the
// display name, the number's digits, or — Telegram-style — any message
// body in the conversation.
const query = ref("");
const filteredConversations = computed(() => {
  const q = query.value.trim().toLowerCase();
  if (!q) return conversations.value;
  const qDigits = q.replace(/\D/g, "");
  // Conversations whose ANY message body matches.
  const bodyHits = new Set<string>();
  for (const m of allMessages.value) {
    if (m.body.toLowerCase().includes(q)) bodyHits.add(convKey(m.address));
  }
  return conversations.value.filter(
    (c) =>
      c.name.toLowerCase().includes(q) ||
      (qDigits.length > 0 && c.address.replace(/\D/g, "").includes(qDigits)) ||
      bodyHits.has(convKey(c.address)),
  );
});

// ---- Virtualised conversation list (same as Contacts/Recents) ----
// Only the rows on screen are in the DOM, so thousands of conversations
// scroll smoothly. Fixed row height (avatar + two lines).
const CONV_ROW_H = 64;
const {
  list: convList,
  containerProps: convContainer,
  wrapperProps: convWrapper,
  scrollTo: convScrollTo,
} = useVirtualList(filteredConversations, { itemHeight: CONV_ROW_H, overscan: 6 });
watch(query, () => convScrollTo(0));
// Thread: bottom-anchored TAIL window — only the newest N bubbles hit the
// DOM; scrolling up first reveals older messages from LOCAL data (instant),
// and only when local is exhausted does the BLE page-fetch fallback fire.
const TAIL_STEP = 60;
const tailCount = ref(TAIL_STEP);
const renderedMessages = computed(() => activeMessages.value.slice(-tailCount.value));
const localHasMore = computed(() => activeMessages.value.length > tailCount.value);

// Telegram-style day separators: a centered date chip wherever the calendar
// day changes between consecutive rendered bubbles.
type ThreadItem = { kind: "date"; key: string; label: string } | { kind: "msg"; m: SmsMessage };
function dayLabel(ms: number): string {
  const d = new Date(ms);
  const today = new Date();
  const yest = new Date(today);
  yest.setDate(today.getDate() - 1);
  const same = (a: Date, b: Date) =>
    a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
  if (same(d, today)) return t("messages.today");
  if (same(d, yest)) return t("recents.yesterday");
  return d.toLocaleDateString(undefined, { day: "numeric", month: "long", year: d.getFullYear() === today.getFullYear() ? undefined : "numeric" });
}
const threadItems = computed<ThreadItem[]>(() => {
  const out: ThreadItem[] = [];
  let lastDay = "";
  for (const m of renderedMessages.value) {
    const day = new Date(m.date).toDateString();
    if (day !== lastDay) {
      lastDay = day;
      out.push({ kind: "date", key: `day-${day}`, label: dayLabel(m.date) });
    }
    out.push({ kind: "msg", m });
  }
  return out;
});

// BLE pagination is pure overhead once the LAN history covers this thread —
// every page would come back as already-known duplicates.
const activeCovered = computed(() =>
  activeNumber.value ? threadCoveredByHistory(activeNumber.value) : false,
);
const activeHasMore = computed(() => {
  if (!activeNumber.value) return false;
  return localHasMore.value || (!activeCovered.value && threadHasMore(activeNumber.value));
});
const activeLoading = computed(() => (activeNumber.value ? threadLoading(activeNumber.value) : false));

function threadFor(address: string): number {
  const d = convKey(address);
  return sms.value.find((m) => convKey(m.address) === d)?.thread ?? 0;
}
function open(address: string) {
  // PUSH on purpose: the chat becomes a history entry, so back (header
  // arrow or mouse button) closes it. The arrow below POPS that entry —
  // pushing /messages here instead would build [list, A, list] and make
  // repeated back presses ping-pong between the chat and the list.
  void router.push(`/messages/${encodeURIComponent(address)}`);
}
function back() {
  router.back();
}
// Per-thread setup runs off the ROUTE (covers open(), ?to= redirects, and
// history navigation alike): reset the render window, restore the draft
// (Telegram-style — leaving a chat keeps what you typed), mark read, and
// fire the BLE page-0 fetch only when the LAN history doesn't already
// cover the thread (LAN-less fallback).
watch(
  activeNumber,
  (address) => {
    if (!address) return;
    tailCount.value = TAIL_STEP;
    draft.value = getDraft(address);
    markConversationRead(address);
    if (!threadCoveredByHistory(address)) {
      void loadThread(address, threadFor(address));
    }
  },
  { immediate: true },
);
// Persist the draft as it's typed (cleared on send).
watch(draft, (text) => {
  if (activeNumber.value) setDraft(activeNumber.value, text);
});
// Tell the backend which chat is on screen so it can mute desktop popups
// for this sender while we're already reading it (cleared on leave/unmount).
watch(
  activeName,
  (name) => {
    void invoke("set_active_chat", { name: activeNumber.value ? name : "" });
  },
  { immediate: true },
);
onUnmounted(() => void invoke("set_active_chat", { name: "" }));
async function send() {
  const body = draft.value.trim();
  if (!body || !activeNumber.value) return;
  draft.value = "";
  await sendSms(activeNumber.value, body);
  await nextTick();
  threadEl.value?.scrollTo({ top: threadEl.value.scrollHeight });
}

// Open a conversation when navigated from Contacts (?to=number) — redirect
// into the canonical thread route.
watch(
  () => route.query.to,
  (to) => {
    if (typeof to === "string" && to) {
      void router.replace(`/messages/${encodeURIComponent(to)}`);
    }
  },
  { immediate: true },
);

// Infinite scroll up: reveal older LOCAL messages first (instant, just a
// bigger render window), then fall back to BLE page-fetch when local data
// is exhausted AND the LAN history doesn't cover the thread. Either way the
// reading position is preserved when content is prepended.
const prependFromHeight = ref<number | null>(null);
// TG-style jump-to-bottom affordance: shown while the user is scrolled up.
const atBottom = ref(true);
function jumpToBottom() {
  const el = threadEl.value;
  if (el) el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
}
function onThreadScroll() {
  const el = threadEl.value;
  if (el) atBottom.value = el.scrollHeight - el.scrollTop - el.clientHeight < 200;
  if (!el || !activeNumber.value || !activeHasMore.value) return;
  if (el.scrollTop <= 64) {
    prependFromHeight.value = el.scrollHeight;
    if (localHasMore.value) {
      tailCount.value += TAIL_STEP;
    } else if (!activeCovered.value) {
      void loadMoreThread(activeNumber.value, threadFor(activeNumber.value));
    } else {
      prependFromHeight.value = null; // nothing to load after all
    }
  }
}
// Opening a chat always lands at the NEWEST message (Telegram-style),
// instantly — including switching straight from another chat, where the
// content-change watch below can't tell "new chat" from "same chat
// updated" (prev isn't empty) and would otherwise leave the view at the top.
watch(activeNumber, async (n) => {
  if (!n) return;
  await nextTick();
  const el = threadEl.value;
  if (el) el.scrollTop = el.scrollHeight;
  // Telegram-style: the composer is ready to type the moment a chat
  // opens — no click needed. (No-op for no-reply senders: the
  // textarea isn't rendered there.)
  composerEl.value?.focus();
}, { immediate: true });

// Scroll management on content changes:
//  - older messages prepended (local reveal or BLE page) → restore the
//    reading position so the view doesn't jump;
//  - otherwise snap to the newest message ONLY when the user is already at
//    (or near) the bottom — a sync arriving while they read old messages
//    must NOT yank them down (the "page pulls" bug).
watch(renderedMessages, async (_now, prev) => {
  const el = threadEl.value;
  if (!el) return;
  const nearBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 150;
  const wasEmpty = (prev?.length ?? 0) === 0;
  await nextTick();
  if (prependFromHeight.value !== null) {
    el.scrollTop = el.scrollHeight - prependFromHeight.value;
    prependFromHeight.value = null;
  } else if (nearBottom || wasEmpty) {
    el.scrollTo({ top: el.scrollHeight });
  }
});

function clockTime(ms: number): string {
  return new Date(ms).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
}
function bubbleClass(m: SmsMessage): string {
  if (m.status === "failed") return "bg-rose-500 text-white rounded-br-sm";
  // Sent: bright emerald reads fine on the light theme but glares on the
  // dark one — there it drops to a deep desaturated green (the WhatsApp-
  // dark approach) that sits comfortably on the near-black canvas.
  if (m.type === 2) return "bg-emerald-500 text-white dark:bg-emerald-900 dark:text-emerald-50 rounded-br-sm";
  // Received: --card sits only ~2% above the dark background — invisible.
  // Lift it to the accent surface there (with a whisper of a border for
  // shape); light theme keeps the card+border look.
  return "bg-card border border-border dark:bg-accent dark:border-white/5 rounded-bl-sm";
}
</script>

<template>
  <div class="h-full flex flex-col">
    <!-- Conversation list -->
    <template v-if="!activeNumber">
      <header class="flex items-center gap-2 px-5 py-4 border-b border-border bg-card/30">
        <MessageSquare class="h-5 w-5 text-muted-foreground" />
        <h1 class="text-base font-semibold">{{ t("messages.title") }}</h1>
        <span v-if="conversations.length" class="ml-auto text-xs text-muted-foreground">
          {{ conversations.length }}
        </span>
      </header>

      <!-- Search (matches name, number digits, or any message body) -->
      <div class="px-4 pt-3 pb-2">
        <SearchInput v-model="query" :placeholder="t('messages.search')" />
      </div>

      <!-- Virtualised conversation list. No padding on the scroll container
           — useVirtualList maps scrollTop straight to row offsets. -->
      <main v-bind="convContainer" class="flex-1 min-h-0 overflow-y-auto">
        <div v-bind="convWrapper">
          <div
            v-for="{ data: c } in convList"
            :key="c.address"
            class="px-3 py-1"
            :style="{ height: CONV_ROW_H + 'px' }"
          >
            <ConversationRow :conv="c" @open="open" />
          </div>
        </div>

        <div
          v-if="smsLoaded && filteredConversations.length === 0"
          class="flex flex-col items-center justify-center text-center text-muted-foreground py-16"
        >
          <MessageSquare class="h-10 w-10 mb-3 opacity-40" />
          <p class="text-sm">{{ query ? t("recents.noResults") : t("messages.empty") }}</p>
        </div>
      </main>
    </template>

    <!-- Thread view + composer -->
    <template v-else>
      <header class="flex items-center gap-2 px-3 py-3 border-b border-border bg-card/30">
        <button class="h-8 w-8 rounded-md flex items-center justify-center hover:bg-accent" @click="back">
          <ArrowLeft class="h-4 w-4" />
        </button>
        <Avatar :name="activeName" :size="28" />
        <h1 class="flex-1 text-sm font-semibold truncate">{{ activeName }}</h1>
        <!-- Call this number (numeric addresses only — alphanumeric sender
             IDs can't be dialled, same gate as the composer) -->
        <button
          v-if="canReply && activeNumber"
          :title="t('contacts.call')"
          class="h-8 w-8 shrink-0 rounded-full flex items-center justify-center text-muted-foreground/60 hover:bg-emerald-500/15 hover:text-emerald-500 transition-colors"
          @click="dial(activeNumber)"
        >
          <Phone class="h-4 w-4" />
        </button>
      </header>

      <div class="relative flex-1 min-h-0 flex flex-col">
      <!-- Jump to newest (TG-style) — floats over the thread while scrolled up -->
      <button
        v-if="!atBottom"
        class="absolute bottom-3 right-3 z-10 h-9 w-9 rounded-full bg-card border border-border shadow-lg flex items-center justify-center text-muted-foreground hover:text-foreground hover:bg-accent transition-colors"
        @click="jumpToBottom"
      >
        <ArrowDown class="h-4 w-4" />
      </button>
      <main ref="threadEl" class="flex-1 min-h-0 overflow-y-auto p-3 space-y-2 flex flex-col" @scroll.passive="onThreadScroll">
        <div v-if="activeLoading" class="flex justify-center py-1 shrink-0">
          <Loader2 class="h-4 w-4 animate-spin text-muted-foreground/60" />
        </div>
        <template v-for="item in threadItems" :key="item.kind === 'date' ? item.key : item.m.id">
          <!-- Telegram-style day separator chip -->
          <div v-if="item.kind === 'date'" class="self-center shrink-0 my-1">
            <span class="text-xs text-muted-foreground bg-card dark:bg-accent/60 border border-border/50 rounded-full px-3 py-1">
              {{ item.label }}
            </span>
          </div>

          <div
            v-else
            class="flex items-center gap-1.5 max-w-[80%]"
            :class="item.m.type === 2 ? 'self-end' : 'self-start'"
          >
          <!-- Failed-send actions (left of the bubble, Redmi-style) -->
          <template v-if="item.m.status === 'failed'">
            <button
              :title="t('messages.resend')"
              class="h-7 w-7 shrink-0 rounded-full flex items-center justify-center text-muted-foreground hover:bg-accent hover:text-emerald-500 transition-colors"
              @click="resendSms(item.m.id)"
            >
              <RotateCw class="h-3.5 w-3.5" />
            </button>
            <button
              :title="t('messages.delete')"
              class="h-7 w-7 shrink-0 rounded-full flex items-center justify-center text-muted-foreground hover:bg-accent hover:text-rose-500 transition-colors"
              @click="deleteLocalSent(item.m.id)"
            >
              <Trash2 class="h-3.5 w-3.5" />
            </button>
          </template>

          <div
            class="relative rounded-2xl px-3 py-2 min-w-0"
            :class="[bubbleClass(item.m), item.m.status === 'sending' ? 'opacity-70' : '']"
          >
            <!-- TG-style timestamp: anchored bottom-right; the invisible
                 trailing spacer reserves that corner on the LAST text line,
                 so a short line sits beside the time and a long one wraps
                 above it — text never runs into the clock. -->
            <template v-if="!item.m.status">
              <!-- pb keeps a TG-like breathing band between the last text
                   line and the clock floating below it -->
              <p class="text-sm whitespace-pre-wrap break-words pb-1.5 selectable-text">{{ item.m.body }}<span class="inline-block w-12" /></p>
              <span class="absolute bottom-1 right-3 text-[10px] opacity-70">{{ clockTime(item.m.date) }}</span>
            </template>
            <!-- Transient states (sending/failed) keep the roomier two-row
                 layout — their labels are wider than a clock. -->
            <template v-else>
              <p class="text-sm whitespace-pre-wrap break-words selectable-text">{{ item.m.body }}</p>
              <p class="text-[10px] mt-0.5 opacity-70 text-right flex items-center justify-end gap-1">
                <Loader2 v-if="item.m.status === 'sending'" class="h-3 w-3 animate-spin" />
                <AlertCircle v-else class="h-3 w-3" />
                <span>{{ item.m.status === "sending" ? t("messages.sending") : t("messages.failed") }}</span>
              </p>
            </template>
          </div>
          </div>
        </template>
        <div v-if="activeMessages.length === 0" class="text-center text-xs text-muted-foreground py-8">
          {{ t("messages.start") }}
        </div>
      </main>
      </div>

      <!-- No-reply sender (alphanumeric ID) — composer replaced with a note -->
      <div
        v-if="!canReply"
        class="border-t border-border bg-card/30 px-4 py-3 text-center text-xs text-muted-foreground"
      >
        {{ t("messages.noReply") }}
      </div>

      <!-- Composer -->
      <div v-else class="relative border-t border-border bg-card/30">
        <!-- Emoji picker popover (emoji are plain Unicode — they ride SMS
             fine; real image stickers would need MMS, deferred) -->
        <div
          v-if="emojiOpen"
          class="emoji-zone absolute bottom-full right-2 mb-2 z-10 rounded-xl overflow-hidden shadow-xl border border-border bg-card"
          @mouseenter="emojiEnter"
          @mouseleave="emojiLeave"
        >
          <emoji-picker
            :class="isDark ? 'dark' : 'light'"
            :data-source="emojiDataUrl"
            style="
              width: min(92vw, 340px);
              height: 400px;
              --background: hsl(var(--card));
              --border-color: hsl(var(--border));
              --indicator-color: hsl(var(--primary));
              --border-radius: 0;
              /* Search field matched to our SearchInput look: 1px border-border,
                 8px radius, ~h-9 with px-3, text-sm, ring-colored focus. */
              --input-border-color: hsl(var(--border));
              --input-border-size: 1px;
              --input-border-radius: 0.5rem;
              --input-padding: 0.5rem 0.75rem;
              --input-font-size: 0.875rem;
              --input-line-height: 1.25rem;
              --input-font-color: hsl(var(--foreground));
              --input-placeholder-color: hsl(var(--muted-foreground));
              --outline-color: hsl(var(--ring));
              --outline-size: 2px;
            "
            @emoji-click="onEmoji"
          />
        </div>
        <form class="flex items-end gap-2 p-3" @submit.prevent="send">
          <textarea
            ref="composerEl"
            v-model="draft"
            rows="1"
            :placeholder="t('messages.compose')"
            class="flex-1 resize-none max-h-28 rounded-xl border border-border bg-muted/50 px-3.5 py-2.5 text-[13.5px] outline-none placeholder:text-muted-foreground transition-colors focus:border-primary"
            @keydown.enter.exact.prevent="send"
          />
          <!-- Telegram layout: smiley sits right of the input, panel above it -->
          <button
            type="button"
            :title="t('messages.emoji')"
            class="emoji-zone h-9 w-9 shrink-0 rounded-full flex items-center justify-center text-muted-foreground hover:bg-accent hover:text-amber-500 transition-colors"
            :class="emojiOpen ? 'bg-accent text-amber-500' : ''"
            @mouseenter="emojiEnter"
            @mouseleave="emojiLeave"
            @click="emojiOpen = !emojiOpen"
          >
            <Smile class="h-5 w-5" />
          </button>
          <button
            type="submit"
            :disabled="!draft.trim()"
            class="h-9 w-9 shrink-0 rounded-full flex items-center justify-center bg-emerald-500 text-white disabled:opacity-40 hover:bg-emerald-600 transition-colors"
          >
            <Send class="h-4 w-4" />
          </button>
        </form>
      </div>
    </template>
  </div>
</template>
