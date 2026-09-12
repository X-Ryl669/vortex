<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from "vue";
import { useI18n } from "vue-i18n";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  ChevronRight,
  Download,
  Eye,
  EyeOff,
  File as FileIcon,
  Folder,
  RefreshCw,
} from "lucide-vue-next";

const { t } = useI18n();

type Entry = {
  id: string;
  name: string;
  mime: string;
  bytes: number;
  modified: number;
  dir: boolean;
};
type Listing = {
  at: string;
  entries: Entry[];
  truncated: boolean;
  error: string | null;
};

const entries = ref<Entry[]>([]);
const loading = ref(true);
const error = ref<string | null>(null);
const truncated = ref(false);
// Where we are, as { id, name } — index 0 is the roots. The phone addresses a
// folder by an opaque document URI, so the trail is what makes "up" possible.
const trail = ref<{ id: string; name: string }[]>([]);
// Files already asked for, so a second click cannot queue the same pull twice.
const requested = ref<Set<string>>(new Set());

let stop: UnlistenFn | undefined;
let waitForGrant: ReturnType<typeof setInterval> | undefined;

// Folders already seen this session, so stepping back into one is instant.
//
// A listing is a round-trip to the phone, and without this every "back" —
// the one navigation people do fastest and most often — emptied the list and
// spun until the phone answered, for a folder whose contents were on screen a
// second earlier. Shown straight away and refreshed behind it: stale for a
// moment beats blank, and the refresh lands before it matters.
const cache = new Map<string, Entry[]>();
// Long enough that browsing feels instant, short enough that a folder someone
// is actively changing on the phone is not remembered wrongly for long.
const CACHE_MS = 60_000;
const cachedAt = new Map<string, number>();

function cachedEntries(at: string): Entry[] | undefined {
  const t = cachedAt.get(at);
  if (t === undefined || Date.now() - t > CACHE_MS) return undefined;
  return cache.get(at);
}

// "No folder shared yet" is the one empty state someone is actively doing
// something about: they are on their phone granting one right now. Nothing
// tells us when that lands, so ask again while that screen is up — and only
// while it is, because every other state is either correct or being read.
const GRANT_POLL_MS = 4000;
function pollWhileUngranted(showingNothing: boolean) {
  clearInterval(waitForGrant);
  waitForGrant = undefined;
  if (!showingNothing) return;
  waitForGrant = setInterval(() => invoke("browse_phone", { at: "" }), GRANT_POLL_MS);
}

function open(at: string, force = false) {
  const hit = force ? undefined : cachedEntries(at);
  if (hit) {
    entries.value = hit;
    error.value = null;
    truncated.value = false;
    loading.value = false;
  } else {
    entries.value = [];
    loading.value = true;
    error.value = null;
  }
  // Asked for either way — the cache decides what is on screen while we wait,
  // not whether to check.
  invoke("browse_phone", { at });
}

function enter(e: Entry) {
  clearInterval(waitForGrant);
  waitForGrant = undefined;
  trail.value.push({ id: e.id, name: e.name });
  // Same URL, one more entry: the router stays unaware (so it will not
  // navigate) while the back gesture gains something of ours to pop.
  history.pushState({ vortexDepth: trail.value.length }, "");
  open(e.id);
}

function upTo(depth: number) {
  // Rewind history by the same number of levels rather than jumping straight
  // there: otherwise a breadcrumb click would leave our pushed entries behind
  // and the next back gesture would walk folders the user had already left.
  const back = trail.value.length - depth;
  if (back > 0) {
    history.go(-back);
    return; // `onPopState` applies it
  }
  applyDepth(depth);
}

function fetchFile(e: Entry) {
  if (e.dir || requested.value.has(e.id)) return;
  requested.value.add(e.id);
  invoke("fetch_phone_file", { id: e.id, name: e.name, mime: e.mime, bytes: e.bytes });
}

// Dot-prefixed entries are hidden, as every file manager hides them: a phone's
// Download folder is full of `.temp_mivideo`-style scratch directories its apps
// made, and they crowd out the two or three things the user came to find.
// Toggled rather than dropped, because "it is not in the list" and "it is not
// there" have to be tellable apart.
const showHidden = ref(false);
const hiddenCount = computed(() => entries.value.filter((e) => e.name.startsWith(".")).length);

// Folders first, then files, each A–Z and case-blind — the order every file
// manager uses, and the reason the raw provider order was unreadable: it comes
// back in whatever order the phone's index happens to hold, so a folder sat
// between two downloads with nothing to tell them apart at a glance.
const sorted = computed(() =>
  entries.value
    .filter((e) => showHidden.value || !e.name.startsWith("."))
    .sort((a, b) => {
      if (a.dir !== b.dir) return a.dir ? -1 : 1;
      return a.name.localeCompare(b.name, undefined, { sensitivity: "base", numeric: true });
    }),
);

// Same shape as the size column: short, and absent rather than wrong when the
// phone reports no timestamp.
function humanDate(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  if (Number.isNaN(d.getTime())) return "";
  const today = new Date();
  const sameYear = d.getFullYear() === today.getFullYear();
  return d.toLocaleDateString(undefined, {
    day: "numeric",
    month: "short",
    year: sameYear ? undefined : "numeric",
  });
}

function humanSize(n: number): string {
  if (n <= 0) return "";
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(0)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / 1024 / 1024).toFixed(1)} MB`;
  return `${(n / 1024 / 1024 / 1024).toFixed(2)} GB`;
}

// Going "back" while browsing means the folder above, not the page before.
//
// Not by swallowing the gesture: the webview starts that navigation below the
// DOM, so preventDefault on the mouse event does nothing and the first click
// several folders deep still threw the user out to whatever page they came
// from. Descending pushes a history entry instead, which makes every back
// gesture — mouse thumb button, Alt+Left, a trackpad swipe — pop one folder
// for free, and leaves the page only once there are no folders left to climb,
// where "back" really does mean the previous page.
function onPopState(ev: PopStateEvent) {
  const depth = (ev.state as { vortexDepth?: number } | null)?.vortexDepth ?? 0;
  if (depth >= trail.value.length) return;
  applyDepth(depth);
}
function onKeyBack(ev: KeyboardEvent) {
  const target = ev.target as HTMLElement | null;
  // Never steal a keystroke someone is typing into something.
  if (target && /^(INPUT|TEXTAREA)$/.test(target.tagName)) return;
  if (ev.key === "Backspace" && trail.value.length) {
    ev.preventDefault();
    history.back();
  }
}

/** Re-read the folder on screen, ignoring what we remember of it — which is
 *  the whole point of a refresh button. */
function refresh() {
  const at = trail.value.length ? trail.value[trail.value.length - 1].id : "";
  cachedAt.delete(at);
  open(at, true);
}

/** Show the folder at `depth` without touching history (the caller did). */
function applyDepth(depth: number) {
  trail.value = trail.value.slice(0, depth);
  open(trail.value.length ? trail.value[trail.value.length - 1].id : "");
}

onMounted(async () => {
  window.addEventListener("popstate", onPopState);
  window.addEventListener("keydown", onKeyBack);
  stop = await listen<Listing>("vortex:phone_files", (ev) => {
    const l = ev.payload;
    // Ignore an answer for a folder we have already navigated away from.
    const here = trail.value.length ? trail.value[trail.value.length - 1].id : "";
    if (l.at !== here) return;
    entries.value = l.entries ?? [];
    truncated.value = !!l.truncated;
    error.value = l.error ?? null;
    loading.value = false;
    // Remember only a real answer: a refusal is about the moment, not the
    // folder, and caching it would keep showing the error after it was fixed.
    if (!l.error) {
      cache.set(l.at, entries.value);
      cachedAt.set(l.at, Date.now());
    }
    pollWhileUngranted(!trail.value.length && !entries.value.length && !error.value);
  });
  open("");
});

onUnmounted(() => {
  window.removeEventListener("popstate", onPopState);
  window.removeEventListener("keydown", onKeyBack);
  stop?.();
  clearInterval(waitForGrant);
  // The phone stops being asked the moment nobody is looking.
  invoke("stop_browsing_phone");
});
</script>

<template>
  <div class="flex h-full flex-col">
    <header class="flex items-center gap-2 border-b border-border px-4 py-3">
      <h1 class="text-base font-semibold">{{ t("phoneFiles.title") }}</h1>
      <button
        v-if="hiddenCount"
        class="ml-auto rounded-md p-1.5 hover:bg-accent"
        :class="showHidden ? 'text-foreground' : 'text-muted-foreground'"
        :title="t(showHidden ? 'phoneFiles.hideHidden' : 'phoneFiles.showHidden')"
        @click="showHidden = !showHidden"
      >
        <Eye v-if="showHidden" class="size-4" />
        <EyeOff v-else class="size-4" />
      </button>
      <button
        class="rounded-md p-1.5 text-muted-foreground hover:bg-accent"
        :class="!hiddenCount && 'ml-auto'"
        :title="t('phoneFiles.refresh')"
        @click="refresh()"
      >
        <RefreshCw class="size-4" :class="loading && 'animate-spin'" />
      </button>
    </header>

    <!-- Breadcrumb. Index 0 is the set of folders the phone has granted. -->
    <nav class="flex flex-wrap items-center gap-1 px-4 py-2 text-sm text-muted-foreground">
      <button class="hover:text-foreground" @click="upTo(0)">{{ t("phoneFiles.roots") }}</button>
      <template v-for="(crumb, i) in trail" :key="crumb.id">
        <ChevronRight class="size-3.5 shrink-0" />
        <button class="truncate hover:text-foreground" @click="upTo(i + 1)">{{ crumb.name }}</button>
      </template>
    </nav>

    <div class="min-h-0 flex-1 overflow-y-auto pb-4">
      <p v-if="error" class="px-2 py-8 text-center text-sm text-muted-foreground">
        {{ error }}
      </p>
      <p v-else-if="loading" class="px-2 py-8 text-center text-sm text-muted-foreground">
        {{ t("phoneFiles.loading") }}
      </p>
      <p v-else-if="!entries.length" class="px-2 py-8 text-center text-sm text-muted-foreground">
        {{ trail.length ? t("phoneFiles.empty") : t("phoneFiles.noFolders") }}
      </p>

      <template v-else>
        <!-- Fixed columns, so a name never pushes the size out of line and the
             eye can run straight down each one. Dates and sizes are tabular
             and right-aligned for the same reason. -->
        <div
          class="sticky top-0 z-10 grid grid-cols-[1fr_5.5rem_4.5rem_1.5rem] items-center gap-3
                 border-b border-border bg-background px-3 py-1.5 text-[11px] font-medium
                 uppercase tracking-wide text-muted-foreground"
        >
          <span>{{ t("phoneFiles.colName") }}</span>
          <span class="text-right">{{ t("phoneFiles.colDate") }}</span>
          <span class="text-right">{{ t("phoneFiles.colSize") }}</span>
          <span></span>
        </div>

        <ul>
          <li v-for="e in sorted" :key="e.id">
            <button
              class="grid w-full grid-cols-[1fr_5.5rem_4.5rem_1.5rem] items-center gap-3
                     border-b border-border/40 px-3 py-1.5 text-left hover:bg-accent"
              @click="e.dir ? enter(e) : fetchFile(e)"
              @dblclick.prevent
            >
              <span class="flex min-w-0 items-center gap-2">
                <Folder v-if="e.dir" class="size-4 shrink-0 text-sky-500" />
                <FileIcon v-else class="size-4 shrink-0 text-muted-foreground" />
                <span class="truncate text-sm">{{ e.name }}</span>
              </span>
              <span class="text-right text-xs tabular-nums text-muted-foreground">
                {{ humanDate(e.modified) }}
              </span>
              <span class="text-right text-xs tabular-nums text-muted-foreground">
                {{ e.dir ? "" : humanSize(e.bytes) }}
              </span>
              <ChevronRight v-if="e.dir" class="size-4 text-muted-foreground" />
              <Download
                v-else
                class="size-4"
                :class="requested.has(e.id) ? 'text-primary' : 'text-muted-foreground/50'"
              />
            </button>
          </li>
        </ul>
      </template>

      <p v-if="truncated" class="px-2 py-3 text-center text-xs text-muted-foreground">
        {{ t("phoneFiles.truncated") }}
      </p>
    </div>
  </div>
</template>
