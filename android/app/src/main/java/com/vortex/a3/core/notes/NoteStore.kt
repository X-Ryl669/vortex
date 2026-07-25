package com.vortex.a3.core.notes

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.io.File
import java.util.UUID

/**
 * The phone's local notes/todos store — the full item set (incl. tombstones)
 * persisted as one JSON file in `filesDir`. A flat list is small, so load-all /
 * save-all beats a Room schema. Exposes a [StateFlow] of the LIVE (non-deleted)
 * items for Compose; keeps tombstones internally so the bidirectional sync can
 * still propagate deletes. Mirrors the laptop's `notes.rs`.
 */
object NoteStore {
    private const val FILE = "notes.json"

    private var file: File? = null
    /** Full set incl. tombstones — the sync source of truth. */
    private var all: List<Note> = emptyList()

    private val _notes = MutableStateFlow<List<Note>>(emptyList())
    /** Live (non-deleted) items, newest-edited first — what the UI renders. */
    val notes: StateFlow<List<Note>> = _notes

    fun init(context: Context) {
        if (file != null) return
        val f = File(context.applicationContext.filesDir, FILE)
        file = f
        all = if (f.exists()) Note.listFromBytes(f.readBytes()) else emptyList()
        publish()
    }

    private fun publish() {
        _notes.value = all.filter { !it.deleted }.sortedByDescending { it.updatedAt }
    }

    private fun persist() {
        file?.let { runCatching { it.writeBytes(Note.listToBytes(all)) } }
    }

    private fun now() = System.currentTimeMillis()

    /** Fired after a LOCAL edit (create/upsert/toggle/delete) — the sync layer
     *  pushes the changed set to the peer. NOT fired by [replaceAll] (a merge
     *  from the peer), so an inbound sync never echoes straight back. */
    @Volatile var onLocalEdit: (() -> Unit)? = null

    /** All items incl. tombstones (for the sync layer). */
    fun snapshot(): List<Note> = all

    private fun afterLocalEdit() {
        persist(); publish(); onLocalEdit?.invoke()
    }

    /** Replace the full set incl. tombstones — used by the sync merge. Persists +
     *  publishes but does NOT fire [onLocalEdit] (avoids a push echo). */
    fun replaceAll(items: List<Note>) {
        all = items
        persist(); publish()
    }

    /** Create a blank item and return it (the UI opens it for editing). */
    fun create(kind: String): Note {
        val n = Note(id = UUID.randomUUID().toString(), kind = kind, updatedAt = now())
        all = all + n
        afterLocalEdit()
        return n
    }

    /** Add a todo with text already filled (the to-do add bar) — persisted but
     *  NOT returned for editing (todos are managed inline in the list). */
    fun addTodo(text: String) {
        val t = text.trim()
        if (t.isEmpty()) return
        all = all + Note(id = UUID.randomUUID().toString(), kind = "todo", title = t, updatedAt = now())
        afterLocalEdit()
    }

    /** Create or replace an item by id, stamping a fresh updatedAt. */
    fun upsert(note: Note) {
        val stamped = note.copy(updatedAt = now(), deleted = false)
        all = all.filter { it.id != stamped.id } + stamped
        afterLocalEdit()
    }

    fun toggle(id: String, done: Boolean) {
        all = all.map { if (it.id == id) it.copy(done = done, updatedAt = now()) else it }
        afterLocalEdit()
    }

    fun delete(id: String) {
        all = all.map { if (it.id == id) it.copy(deleted = true, updatedAt = now()) else it }
        afterLocalEdit()
    }
}
