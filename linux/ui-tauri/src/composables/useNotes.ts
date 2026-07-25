import { computed, ref } from "vue";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

/** Mirrors Rust `notes::Item` / Kotlin `core.notes.Note`. One unified item:
 *  a free-text note OR a checkable todo. `updated_at` is the LWW clock the
 *  bidirectional sync merges on; `deleted` tombstones never reach this list
 *  (the backend filters them out). */
export interface Note {
  id: string;
  kind: "note" | "todo";
  title: string;
  body: string;
  done: boolean;
  /** Epoch ms of the todo's reminder, 0 = none. */
  due_at: number;
  updated_at: number;
  deleted: boolean;
}

export type NotesFilter = "all" | "note" | "todo";

export const notes = ref<Note[]>([]);
export const notesLoaded = ref(false);
export const filter = ref<NotesFilter>("all");
/** The id of the item open in the editor pane (null = nothing selected). */
export const selectedId = ref<string | null>(null);

let started = false;

/** Load the cached list and subscribe to backend pushes (the same event the
 *  sync path emits, so a remote edit re-renders live). Idempotent. */
export async function initNotes(): Promise<void> {
  if (!started) {
    started = true;
    await listen<Note[]>("vortex:notes", e => {
      notes.value = e.payload ?? [];
    });
  }
  notes.value = await invoke<Note[]>("get_notes");
  notesLoaded.value = true;
}

/** Newest-edited first; todos with the list, notes too — the UI filters by kind. */
export const visibleNotes = computed(() => {
  const f = filter.value;
  return notes.value
    .filter(n => f === "all" || n.kind === f)
    .slice()
    .sort((a, b) => b.updated_at - a.updated_at);
});

export const selected = computed(() => notes.value.find(n => n.id === selectedId.value) ?? null);

function uuid(): string {
  // Webview crypto — no server, no central id authority (the phone uses java.util.UUID).
  return crypto.randomUUID();
}

/** Create a blank item of the given kind, persist it, and select it for editing. */
export async function createNote(kind: "note" | "todo"): Promise<void> {
  const item: Note = {
    id: uuid(),
    kind,
    title: "",
    body: "",
    done: false,
    due_at: 0,
    updated_at: Date.now(),
    deleted: false,
  };
  selectedId.value = item.id;
  await invoke("upsert_note", { item });
}

/** Persist an edited item (backend stamps a fresh updated_at). */
export async function saveNote(item: Note): Promise<void> {
  await invoke("upsert_note", { item });
}

/** Add a todo straight from the to-do add bar — text in the title, persisted
 *  but NOT selected (todos are managed inline in the list, no editor pane). */
export async function addTodo(text: string): Promise<void> {
  const t = text.trim();
  if (!t) return;
  const item: Note = {
    id: uuid(),
    kind: "todo",
    title: t,
    body: "",
    done: false,
    due_at: 0,
    updated_at: Date.now(),
    deleted: false,
  };
  await invoke("upsert_note", { item });
}

export async function toggleTodo(id: string, done: boolean): Promise<void> {
  await invoke("toggle_todo", { id, done });
}

export async function deleteNote(id: string): Promise<void> {
  if (selectedId.value === id) selectedId.value = null;
  await invoke("delete_note", { id });
}
