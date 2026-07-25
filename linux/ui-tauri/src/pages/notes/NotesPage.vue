<script setup lang="ts">
import { computed, onMounted, ref, watch } from "vue";
import { useI18n } from "vue-i18n";
import { Plus, Check, X, Trash2 } from "lucide-vue-next";
import DateTimePicker from "./DateTimePicker.vue";
import {
  visibleNotes,
  filter,
  selected,
  selectedId,
  initNotes,
  createNote,
  saveNote,
  toggleTodo,
  deleteNote,
  addTodo,
  type Note,
} from "@/composables/useNotes";
import { cn } from "@/lib/utils";

const { t } = useI18n();

// Two segments — Notes | To-dos — drive the shared kind filter. The Notes
// segment shows a list + editor; the To-dos segment shows a checklist + a
// live progress ring (checklist progress-ring style).
const mode = ref<"notes" | "todos">("notes");
function setMode(m: "notes" | "todos") {
  mode.value = m;
  filter.value = m === "notes" ? "note" : "todo";
}

// A LOCAL editing copy so the textarea keeps focus + content while backend
// pushes (a save echo, or a remote sync) replace the list. Re-seeded only when
// the SELECTED item changes id (incl. when a freshly-created item arrives async).
const editing = ref<Note | null>(null);
watch(
  selected,
  (sel) => {
    if (sel && sel.id !== editing.value?.id) editing.value = { ...sel };
    if (!sel) editing.value = null;
  },
  { immediate: true },
);

// Debounced auto-save: type freely, persist ~400ms after the last keystroke.
let saveTimer: ReturnType<typeof setTimeout> | undefined;
function scheduleSave() {
  if (!editing.value) return;
  clearTimeout(saveTimer);
  const snapshot = { ...editing.value };
  saveTimer = setTimeout(() => saveNote(snapshot), 400);
}

// Editor reminder (note pane) — DateTimePicker emits epoch-ms (0 = cleared).
function onDuePicked(ms: number) {
  if (!editing.value) return;
  editing.value.due_at = ms;
  scheduleSave();
}
// Per-row reminder on a to-do (no editor pane in to-do mode) — persist directly.
function setTodoDue(n: Note, ms: number) {
  void saveNote({ ...n, due_at: ms });
}

const newTodo = ref("");
async function addNewTodo() {
  const text = newTodo.value.trim();
  if (!text) return;
  newTodo.value = "";
  await addTodo(text);
}

function relTime(ms: number): string {
  if (!ms) return "";
  const d = new Date(ms);
  const time = d.toLocaleTimeString([], { hour: "2-digit", minute: "2-digit" });
  if (d.toDateString() === new Date().toDateString()) return time;
  return d.toLocaleDateString([], { month: "short", day: "numeric" });
}
function snippet(n: Note): string {
  const s = (n.body || "").replace(/\s+/g, " ").trim();
  return s || t("notes.empty_body");
}

function select(n: Note) {
  selectedId.value = n.id;
}
async function remove(id: string) {
  await deleteNote(id);
}

// To-do progress ring (checklist style). In to-do mode `visibleNotes` IS
// the todo list (filter === "todo"), so the counts read straight off it.
const doneCount = computed(() => visibleNotes.value.filter((n) => n.done).length);
const totalTodos = computed(() => visibleNotes.value.length);
const pct = computed(() => (totalTodos.value ? doneCount.value / totalTodos.value : 0));
const RING_C = 2 * Math.PI * 52;
const ringOffset = computed(() => RING_C * (1 - pct.value));

onMounted(() => {
  void initNotes();
  filter.value = "note";
});
</script>

<template>
  <div class="h-full flex min-h-0">
    <!-- ░░ LEFT COLUMN — list ░░ -->
    <div class="w-80 shrink-0 border-r border-border flex flex-col min-w-0">
      <div class="px-4 pt-5 pb-3">
        <div class="flex items-center justify-between h-8">
          <span class="inline-flex items-center gap-1.5 text-[11px] font-medium text-muted-foreground">
            <span class="h-1.5 w-1.5 rounded-full bg-primary" />{{ t("notes.synced") }}
          </span>
          <button
            v-if="mode === 'notes'"
            class="grid h-8 w-8 place-items-center rounded-[9px] bg-muted/50 border border-border text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
            :title="t('notes.new_note')"
            @click="createNote('note')"
          >
            <Plus class="h-[17px] w-[17px]" :stroke-width="2" />
          </button>
        </div>
        <!-- segment — a sliding highlight pill (sidebar-style) glides between
             the two tabs instead of snapping. -->
        <div class="relative mt-3.5 grid grid-cols-2 rounded-xl bg-muted/40 border border-border p-1">
          <span
            class="pointer-events-none absolute inset-y-1 left-1 w-[calc(50%-4px)] rounded-lg bg-foreground/[0.08] transition-transform duration-300 ease-[cubic-bezier(.22,1,.36,1)]"
            :style="{ transform: mode === 'todos' ? 'translateX(100%)' : 'translateX(0)' }"
          />
          <button
            class="relative z-[1] rounded-lg py-2 text-[13px] font-semibold transition-colors"
            :class="mode === 'notes' ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'"
            @click="setMode('notes')"
          >
            {{ t("notes.notes") }}
          </button>
          <button
            class="relative z-[1] rounded-lg py-2 text-[13px] font-semibold transition-colors"
            :class="mode === 'todos' ? 'text-foreground' : 'text-muted-foreground hover:text-foreground'"
            @click="setMode('todos')"
          >
            {{ t("notes.todos") }}
          </button>
        </div>
      </div>

      <!-- NOTES list -->
      <div v-if="mode === 'notes'" class="flex-1 overflow-y-auto px-3 pb-4 flex flex-col gap-0.5">
        <p v-if="!visibleNotes.length" class="px-3 py-8 text-center text-xs text-muted-foreground">
          {{ t("notes.empty") }}
        </p>
        <button
          v-for="n in visibleNotes"
          :key="n.id"
          :class="cn('flex flex-col gap-0.5 rounded-2xl px-3.5 py-3 text-left transition-colors', selectedId === n.id ? 'bg-foreground/[0.06]' : 'hover:bg-foreground/[0.03]')"
          @click="select(n)"
        >
          <div class="flex items-baseline justify-between gap-2">
            <span class="truncate text-sm font-semibold text-foreground">{{ n.title || t("notes.untitled") }}</span>
            <span class="shrink-0 text-[11px] text-muted-foreground">{{ relTime(n.updated_at) }}</span>
          </div>
          <span class="truncate text-[12.5px] text-muted-foreground">{{ snippet(n) }}</span>
        </button>
      </div>

      <!-- TODOS list + add bar -->
      <template v-else>
        <div class="flex-1 overflow-y-auto px-2.5 pb-2 flex flex-col">
          <p v-if="!visibleNotes.length" class="px-3 py-8 text-center text-xs text-muted-foreground">
            {{ t("notes.empty_todos") }}
          </p>
          <div
            v-for="n in visibleNotes"
            :key="n.id"
            class="group flex items-start gap-3 rounded-xl px-2.5 py-2.5 transition-colors hover:bg-foreground/[0.03]"
          >
            <button
              :class="cn('mt-px grid h-6 w-6 shrink-0 place-items-center rounded-full border-2 transition-all', n.done ? 'border-primary bg-primary text-black shadow-[0_0_14px_rgba(46,204,113,0.35)]' : 'border-muted-foreground/40 hover:border-muted-foreground')"
              @click="toggleTodo(n.id, !n.done)"
            >
              <Check v-if="n.done" class="h-[13px] w-[13px]" :stroke-width="3" />
            </button>
            <div class="min-w-0 flex-1">
              <span :class="cn('text-[14.5px] transition-colors', n.done ? 'text-muted-foreground line-through' : 'text-foreground')">
                {{ n.title || t("notes.untitled") }}
              </span>
            </div>
            <DateTimePicker :model-value="n.due_at" @update:model-value="(ms: number) => setTodoDue(n, ms)" />
            <button
              class="grid h-[26px] w-[26px] shrink-0 place-items-center rounded-lg text-muted-foreground/60 opacity-0 transition-colors hover:bg-foreground/[0.08] hover:text-foreground group-hover:opacity-100"
              :title="t('notes.delete')"
              @click="remove(n.id)"
            >
              <X class="h-[15px] w-[15px]" :stroke-width="2" />
            </button>
          </div>
        </div>
        <!-- add bar -->
        <div class="px-3.5 pb-[18px] pt-2.5">
          <div class="flex items-center gap-2 rounded-full border border-border bg-muted/40 py-1.5 pl-4 pr-1.5">
            <input
              v-model="newTodo"
              :placeholder="t('notes.add_todo')"
              class="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
              @keydown.enter="addNewTodo"
            />
            <button
              class="grid h-9 w-9 shrink-0 place-items-center rounded-full bg-primary text-black shadow-[0_4px_16px_rgba(46,204,113,0.4)] transition-transform hover:scale-105 disabled:opacity-40 disabled:hover:scale-100"
              :disabled="!newTodo.trim()"
              @click="addNewTodo"
            >
              <Plus class="h-[19px] w-[19px]" :stroke-width="2.4" />
            </button>
          </div>
        </div>
      </template>
    </div>

    <!-- ░░ RIGHT PANE ░░ -->
    <!-- Notes editor -->
    <div v-if="mode === 'notes'" class="flex-1 flex flex-col min-w-0">
      <div v-if="editing" class="flex h-full flex-col">
        <div class="flex items-center justify-between px-7 pb-3 pt-5">
          <span class="text-xs text-muted-foreground">{{ relTime(editing.updated_at) }}</span>
          <button
            class="grid h-[34px] w-[34px] place-items-center rounded-[10px] bg-muted/40 border border-border text-muted-foreground hover:bg-muted hover:text-foreground transition-colors"
            :title="t('notes.delete')"
            @click="remove(editing.id)"
          >
            <Trash2 class="h-[17px] w-[17px]" :stroke-width="1.8" />
          </button>
        </div>
        <input
          v-model="editing.title"
          :placeholder="t('notes.title_placeholder')"
          class="bg-transparent px-7 text-2xl font-semibold tracking-[-0.4px] outline-none placeholder:text-muted-foreground/60"
          @input="scheduleSave"
        />
        <textarea
          v-model="editing.body"
          :placeholder="t('notes.body_placeholder')"
          class="mt-3 flex-1 resize-none bg-transparent px-7 pb-7 text-[15px] leading-[1.65] text-muted-foreground outline-none placeholder:text-muted-foreground/60"
          @input="scheduleSave"
        />
      </div>
      <div v-else class="flex flex-1 flex-col items-center justify-center gap-3 text-muted-foreground">
        <Plus class="h-10 w-10 opacity-30" />
        <p class="text-sm">{{ t("notes.pick_or_create") }}</p>
      </div>
    </div>

    <!-- To-dos progress ring -->
    <div v-else class="flex flex-1 flex-col items-center justify-center gap-2 min-w-0">
      <div class="relative h-40 w-40">
        <svg width="160" height="160" viewBox="0 0 140 140">
          <circle cx="70" cy="70" r="52" fill="none" stroke="hsl(var(--border))" stroke-width="11" />
          <circle
            cx="70"
            cy="70"
            r="52"
            fill="none"
            stroke="hsl(var(--primary))"
            stroke-width="11"
            stroke-linecap="round"
            :stroke-dasharray="RING_C.toFixed(1)"
            :stroke-dashoffset="ringOffset.toFixed(1)"
            transform="rotate(-90 70 70)"
            class="ring-fill"
          />
        </svg>
        <div class="absolute inset-0 flex flex-col items-center justify-center">
          <span class="text-[34px] font-bold tracking-[-1px] text-foreground">{{ Math.round(pct * 100) }}%</span>
        </div>
      </div>
      <div class="mt-1.5 text-[15px] font-semibold text-foreground">
        {{ t("notes.todos_done", { done: doneCount, total: totalTodos }) }}
      </div>
      <div class="text-[12.5px] text-muted-foreground">{{ t("notes.tap_complete") }}</div>
    </div>
  </div>
</template>

<style scoped>
.ring-fill {
  transition: stroke-dashoffset 0.6s cubic-bezier(0.22, 1, 0.36, 1);
  filter: drop-shadow(0 0 8px hsl(var(--primary) / 0.4));
}
</style>
