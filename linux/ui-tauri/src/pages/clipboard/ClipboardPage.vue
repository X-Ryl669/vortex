<script setup lang="ts">
// Clipboard-history popup (frameless, transparent, always-on-top —
// GNOME shortcut → Super+V). Launcher-style: the search input is focused
// on open, arrows move the selection, Enter applies it; click works too.
// A right-hand PREVIEW pane (big image / full text + metadata) slides in
// when the selected entry needs more room than the list row gives (image
// or long text) — the Rust side widens the window to match. The list is
// VIRTUALISED (fixed-height rows via useVirtualList) so it stays smooth at
// up to ~1000 entries — only the visible ~10 rows are in the DOM. Esc /
// losing focus hides it (handled in Rust).
import { onMounted, onUnmounted, ref, computed, watch, nextTick } from "vue";
import { useI18n } from "vue-i18n";
import { useVirtualList } from "@vueuse/core";
import { invoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import {
  Pin,
  PinOff,
  Trash2,
  ClipboardList,
  Search,
  Image as ImageIcon,
  Type,
  Clock,
  HardDrive,
} from "lucide-vue-next";

interface ClipEntry {
  id: string;
  kind: "text" | "image";
  text: string | null;
  path: string | null;
  bytes: number;
  ts_ms: number;
  pinned: boolean;
}

// Per-row heights (card + the 8px gap). The virtual list needs a stable
// height per row, but it accepts a FUNCTION — so we give text rows a compact
// height (2-line clamp; the full text is in the preview pane) and image rows
// a taller one for the thumbnail. Deterministic per kind ⇒ virtualisation
// stays exact (no DOM measuring, no scroll drift).
// Sized for a macOS-launcher feel: the panel is now a share of the monitor
// (Rust picks the height), so rows can breathe instead of packing a fixed
// 600px window as tightly as possible.
const TEXT_ROW_H = 52;
const IMG_ROW_H = 76;
function rowH(e: ClipEntry | undefined): number {
  return e?.kind === "image" ? IMG_ROW_H : TEXT_ROW_H;
}

const { t } = useI18n();
const entries = ref<ClipEntry[]>([]);
const filter = ref("");
const selected = ref(0);
const inputEl = ref<HTMLInputElement | null>(null);
const previewEl = ref<HTMLElement | null>(null);
// Full (untruncated) entry for the preview pane — fetched on demand so
// the list itself stays light (its text is capped at 400 chars).
const full = ref<ClipEntry | null>(null);

// Searchable haystack per entry: text content for text, and for images
// the word "image"/"rasm" + size in KB + the capture date — so a search
// finds a screenshot by "rasm", "image", "120kb", or "jun" / a date.
function haystack(e: ClipEntry): string {
  if (e.kind === "text") return (e.text ?? "").toLowerCase();
  const kb = `${Math.round(e.bytes / 1024)}kb`;
  const d = new Date(e.ts_ms);
  const date = `${d.toLocaleDateString(undefined, { day: "numeric", month: "short" })} ${d.toLocaleDateString()}`;
  return `image rasm ${kb} ${date}`.toLowerCase();
}

const visible = computed(() => {
  const q = filter.value.trim().toLowerCase();
  if (!q) return entries.value;
  return entries.value.filter((e) => haystack(e).includes(q));
});

// Virtual window over `visible` — only the rows on screen (plus a little
// overscan) are rendered, so 1000 entries are as cheap as 10.
const {
  list: vlist,
  containerProps,
  wrapperProps,
  scrollTo: vScrollTo,
} = useVirtualList(visible, {
  itemHeight: (i) => rowH(visible.value[i]),
  overscan: 6,
});

const current = computed<ClipEntry | undefined>(() => visible.value[selected.value]);

// The detail pane shows (and the window widens) whenever the entry can't be
// read in full from the list row. The list now shows a SINGLE truncated line
// for text, so anything that won't fit on one line (~45+ chars) or carries a
// newline needs the preview; only genuinely short one-liners stay narrow.
function needsPreview(e: ClipEntry | undefined): boolean {
  if (!e) return false;
  if (e.kind === "image") return true;
  const txt = e.text ?? "";
  return txt.length > 40 || txt.includes("\n");
}

const showPreview = computed(() => needsPreview(current.value));

function fmtSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${Math.round(bytes / 1024)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

function fmtDate(ms: number): string {
  return new Date(ms).toLocaleString(undefined, {
    day: "numeric",
    month: "short",
    year: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

// Widen/narrow the window for the current selection, and (when shown)
// load its full untruncated content into the detail pane. Runs on every
// selection change.
async function syncPreview() {
  const need = showPreview.value;
  void invoke("clipboard_set_preview", { visible: need }).catch(() => {});
  if (!need) {
    full.value = null;
    return;
  }
  const e = current.value;
  if (!e) return;
  try {
    full.value = await invoke<ClipEntry | null>("clipboard_get", { id: e.id });
  } catch {
    full.value = e;
  }
  // New content starts at the top, never mid-scroll from the last entry.
  void nextTick(() => previewEl.value?.scrollTo({ top: 0 }));
}

watch([filter, entries], () => {
  selected.value = 0;
});
watch(current, syncPreview);

async function refresh() {
  try {
    entries.value = await invoke<ClipEntry[]>("clipboard_history");
  } catch {
    /* backend not ready */
  }
}

async function pick(e: ClipEntry | undefined) {
  if (!e) return;
  await invoke("clipboard_select", { id: e.id }).catch(() => {});
  hide();
}

async function togglePin(e: ClipEntry, ev: Event) {
  ev.stopPropagation();
  await invoke("clipboard_pin", { id: e.id, pinned: !e.pinned }).catch(() => {});
  await refresh();
}

async function remove(e: ClipEntry, ev: Event) {
  ev.stopPropagation();
  await invoke("clipboard_delete", { id: e.id }).catch(() => {});
  await refresh();
}

function resetScroll() {
  // useVirtualList tracks its own scroll offset — set it through scrollTo
  // (not a raw scrollTop write, which leaves its internal state stale and
  // the window scrolled to the old row on reopen).
  vScrollTo(0);
}

function hide() {
  // Reset here too (not just on reopen) so a close-then-reopen always
  // lands on the newest entry at the top, even if the reopen's `rearm`
  // somehow races the window becoming visible.
  filter.value = "";
  selected.value = 0;
  resetScroll();
  void invoke("clipboard_hide");
}

// Cumulative pixel offset of a row (rows vary in height by kind, so we sum
// rather than multiply).
function offsetOf(idx: number): number {
  const arr = visible.value;
  let top = 0;
  for (let i = 0; i < idx && i < arr.length; i++) top += rowH(arr[i]);
  return top;
}

// Keep the selected row in view — only scroll when it's above/below the
// viewport ("nearest" behaviour). Setting scrollTop fires the container's
// onScroll, which updates the virtual window.
function scrollToSelected() {
  const c = containerProps.ref.value;
  const cur = visible.value[selected.value];
  if (!c || !cur) return;
  const top = offsetOf(selected.value);
  const bottom = top + rowH(cur);
  if (top < c.scrollTop) c.scrollTop = top;
  else if (bottom > c.scrollTop + c.clientHeight) c.scrollTop = bottom - c.clientHeight;
}

// Rows that fit in the viewport — the jump for a page move (one row of
// overlap kept for context). Uses the compact text height so a page never
// overshoots a screen mostly made of text rows.
function pageSize(): number {
  const c = containerProps.ref.value;
  const h = c ? c.clientHeight : 400;
  return Math.max(1, Math.floor(h / TEXT_ROW_H) - 1);
}

function move(delta: number) {
  const n = visible.value.length;
  if (!n) return;
  selected.value = (selected.value + delta + n) % n;
  scrollToSelected();
}

// Absolute jump, clamped (no wrap) — for Ctrl+Up/Down (ends) and the
// Left/Right page jumps.
function moveTo(idx: number) {
  const n = visible.value.length;
  if (!n) return;
  selected.value = Math.max(0, Math.min(idx, n - 1));
  scrollToSelected();
}

// WebKit fires `mousemove` when the list scrolls under a STATIONARY
// pointer (keyboard nav scrolls the row under the cursor away, a new one
// slides in). Naively binding @mousemove→selected would then yank the
// selection back to whatever row landed under the cursor — so arrow-down
// past the visible edge appeared to jump UP. Only honor a move when the
// pointer coordinates actually changed.
let lastX = -1;
let lastY = -1;
function onHover(i: number, e: MouseEvent) {
  if (e.clientX === lastX && e.clientY === lastY) return;
  lastX = e.clientX;
  lastY = e.clientY;
  selected.value = i;
}

// The window is transparent so the rounded card cuts cleanly; style.css
// paints html/body/#app opaque, so clear all three or the corners show
// solid squares under the radius.
function clearPageBackground() {
  for (const el of [document.documentElement, document.body, document.getElementById("app")]) {
    if (el) (el as HTMLElement).style.setProperty("background", "transparent", "important");
  }
}

// Re-arm on every (re)show: fresh list, reset selection, refocus input.
// On Wayland the webview grabs keyboard focus a beat after the window
// becomes visible, so the FIRST keypress used to be swallowed (Esc /
// arrows needed two presses). Focus now, then again after a short
// delay, so the webview definitely owns the keyboard before any key.
async function rearm() {
  // Reset BEFORE the async refresh so the old selection/scroll never shows
  // for a frame, then again after — hiding (not destroying) the window
  // keeps the old selection + scroll offset otherwise, so it'd reappear on
  // whatever the last-focused entry was instead of the newest at the top.
  selected.value = 0;
  filter.value = "";
  resetScroll();
  // Paint the stored history FIRST so the panel is usable immediately.
  await refresh();
  selected.value = 0;
  await syncPreview();
  await nextTick();
  resetScroll();
  focusSearch();
  // Only THEN capture whatever's on the clipboard right now, with a fresh
  // connection — the background watcher's X connection can go stale, so a
  // just-copied item otherwise only showed after an app restart. It emits
  // `vortex:clipboard` when it finds something new, which refreshes us; it
  // used to be awaited BEFORE the first paint, putting a blocking arboard
  // round-trip in front of every open.
  void invoke("clipboard_capture_now").catch(() => {});
}

// The caret is a two-part problem: the DOM element must take focus AND the
// WINDOW must hold the keyboard (forced from X in Rust, which can land after
// these calls). Retry a few times, and again whenever the window is focused,
// so the search box is never left dead.
function focusSearch() {
  inputEl.value?.focus();
  setTimeout(() => inputEl.value?.focus(), 70);
  setTimeout(() => inputEl.value?.focus(), 220);
}

// Single keyboard source at the WINDOW level (not on the input) so the
// shortcuts work no matter which element holds focus — and exactly once.
//   ↑/↓            move one
//   Ctrl+↑/↓       jump to first / last
//   ←/→            page up / down (one viewport)
//   Enter          apply,  Esc  close
// Left/Right are claimed for paging, so they don't move the text cursor in
// the search box — fine for a quick filter (type to narrow, Esc/Enter to
// act).
function onKey(e: KeyboardEvent) {
  const ctrl = e.ctrlKey || e.metaKey;
  const n = visible.value.length;
  if (e.key === "Escape") {
    e.preventDefault();
    hide();
  } else if (e.key === "ArrowDown") {
    e.preventDefault();
    ctrl ? moveTo(n - 1) : move(1);
  } else if (e.key === "ArrowUp") {
    e.preventDefault();
    ctrl ? moveTo(0) : move(-1);
  } else if (e.key === "ArrowRight") {
    e.preventDefault();
    moveTo(selected.value + pageSize());
  } else if (e.key === "ArrowLeft") {
    e.preventDefault();
    moveTo(selected.value - pageSize());
  } else if (e.key === "Enter") {
    e.preventDefault();
    void pick(visible.value[selected.value]);
  }
}

let unlisten: UnlistenFn | null = null;
let unshown: UnlistenFn | null = null;
onMounted(async () => {
  clearPageBackground();
  // Rust calls this by eval on every (re)show of the reused window — events
  // to a just-shown (waking) webview race and get missed, so the backend
  // drives rearm directly instead.
  (window as unknown as { __vortexRearm?: () => void }).__vortexRearm = () => {
    void rearm();
  };
  await rearm();
  // Keyboard at the window level — single source, any focus. Blur-to-close
  // is handled in Rust (reliable on Wayland).
  window.addEventListener("keydown", onKey);
  window.addEventListener("focus", focusSearch);
  unlisten = await listen("vortex:clipboard", refresh);
  // Kept as a backup trigger; the eval path above is the reliable one.
  unshown = await listen("vortex:clipboard-shown", rearm);
});
onUnmounted(() => {
  window.removeEventListener("keydown", onKey);
  window.removeEventListener("focus", focusSearch);
  unlisten?.();
  unshown?.();
});
</script>

<template>
  <div
    class="h-screen w-screen flex bg-background text-foreground overflow-hidden border border-border rounded-2xl"
  >
    <!-- LEFT: search + virtualised list. ALWAYS a fixed 460px (never flex)
         so toggling the detail pane can't reflow it by even a sub-pixel —
         narrow window == this column exactly; wide window == this column +
         the pane. The border-r is always present (invisible at the window
         edge when narrow) so it claims no extra space when the pane opens. -->
    <div class="flex flex-col w-[460px] shrink-0 min-w-0 min-h-0 border-r border-border">
      <header class="flex items-center gap-2 px-4 py-2.5 border-b border-border bg-card shrink-0">
        <ClipboardList class="h-4 w-4 text-muted-foreground" />
        <span class="text-sm font-semibold flex-1">{{ t("clipboard.title") }}</span>
        <span class="text-[11px] text-muted-foreground">{{ entries.length }}</span>
      </header>

      <div class="px-4 pt-3 pb-2 shrink-0 border-b border-border/50">
        <div class="flex items-center gap-2 h-10 px-3 rounded-lg border border-border bg-card">
          <Search class="h-4 w-4 text-muted-foreground/70 shrink-0" />
          <input
            ref="inputEl"
            v-model="filter"
            :placeholder="t('clipboard.search')"
            class="flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground/50"
          />
        </div>
      </div>

      <!-- No padding on the scroll container: useVirtualList maps scrollTop
           directly to row offsets, so a padding-top would shift everything
           and clip the last row. Each row's py-1 gives the gap + the top /
           bottom breathing room. -->
      <main v-bind="containerProps" class="flex-1 min-h-0 overflow-y-auto">
        <p v-if="visible.length === 0" class="text-xs text-muted-foreground text-center py-8">
          {{ t("clipboard.empty") }}
        </p>
        <div v-else v-bind="wrapperProps">
          <div
            v-for="{ data: e, index: i } in vlist"
            :key="e.id"
            class="px-4 py-1"
            :style="{ height: rowH(e) + 'px' }"
          >
            <button
              class="group h-full w-full text-left rounded-lg border bg-card px-3 relative overflow-hidden flex items-center"
              :class="i === selected
                ? 'border-primary ring-1 ring-primary/60 bg-accent'
                : 'border-border hover:bg-accent/60'"
              @click="pick(e)"
              @mousemove="onHover(i, $event)"
            >
              <!-- Compact image row: a small left-aligned thumbnail (its
                   shape still hints which screenshot) + size — the full image
                   lives in the right preview pane, so the list stays tight. -->
              <div v-if="e.kind === 'image' && e.path" class="flex items-center gap-2.5 w-full">
                <img
                  :src="convertFileSrc(e.path)"
                  class="rounded object-contain max-h-[58px] max-w-[104px] w-auto h-auto shrink-0 border border-border/50 bg-background"
                  loading="lazy"
                />
                <span class="text-[12px] text-muted-foreground flex items-center gap-1.5">
                  <ImageIcon class="h-3.5 w-3.5 shrink-0" />
                  {{ t("clipboard.image") }} · {{ fmtSize(e.bytes) }}
                </span>
              </div>
              <!-- Compact text row: a single truncated line (the full text is
                   in the preview pane), slightly larger for readability. -->
              <p
                v-else
                class="text-sm truncate pr-10"
              >{{ e.text }}</p>
              <!-- Pin / delete at the END (bottom-right), revealed on hover
                   or when pinned. Absolute so they don't push the content. -->
              <span
                class="absolute bottom-1.5 right-2 opacity-0 group-hover:opacity-100 transition-opacity flex items-center gap-1.5"
                :class="{ 'opacity-100': e.pinned }"
              >
                <component
                  :is="e.pinned ? PinOff : Pin"
                  class="h-4 w-4 text-muted-foreground hover:text-foreground"
                  :class="{ 'text-amber-500': e.pinned }"
                  @click="togglePin(e, $event)"
                />
                <Trash2
                  class="h-4 w-4 text-muted-foreground hover:text-red-500"
                  @click="remove(e, $event)"
                />
              </span>
            </button>
          </div>
        </div>
      </main>
    </div>

    <!-- RIGHT: detail pane — shown when the entry needs the room. The
         window itself widens (Rust) so this never overlaps the list. -->
    <aside v-if="showPreview && full" class="flex-1 min-w-0 flex flex-col bg-background">
      <!-- Image: centred vertically + horizontally; text: top-aligned so it
           reads from the top. -->
      <div
        ref="previewEl"
        class="flex-1 min-h-0 overflow-auto p-4 flex justify-center"
        :class="full.kind === 'image' ? 'items-center' : 'items-start'"
      >
        <img
          v-if="full.kind === 'image' && full.path"
          :src="convertFileSrc(full.path)"
          class="w-full h-auto max-h-full rounded-lg border border-border object-contain"
        />
        <p
          v-else
          class="w-full text-xs leading-relaxed whitespace-pre-wrap [overflow-wrap:anywhere] font-mono"
        >{{ full.text }}</p>
      </div>

      <footer
        class="shrink-0 border-t border-border px-5 py-3 flex items-center gap-4 text-[12px] text-muted-foreground bg-card/40"
      >
        <span class="flex items-center gap-1.5">
          <component :is="full.kind === 'image' ? ImageIcon : Type" class="h-3.5 w-3.5" />
          {{ full.kind === "image" ? t("clipboard.image") : `${(full.text ?? '').length} ${t("clipboard.chars")}` }}
        </span>
        <span class="flex items-center gap-1.5">
          <HardDrive class="h-3.5 w-3.5" />
          {{ fmtSize(full.bytes) }}
        </span>
        <span class="flex items-center gap-1.5 ml-auto whitespace-nowrap">
          <Clock class="h-3.5 w-3.5" />
          {{ fmtDate(full.ts_ms) }}
        </span>
      </footer>
    </aside>
  </div>
</template>
