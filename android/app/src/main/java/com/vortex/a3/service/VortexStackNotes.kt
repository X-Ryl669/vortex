package com.vortex.a3.service

import com.vortex.a3.core.notes.NoteReminderScheduler
import com.vortex.a3.core.notes.NoteStore
import com.vortex.a3.core.notes.NoteSync
import kotlinx.coroutines.launch

/**
 * Wires the notes/todos bidirectional sync (NOTES_SYNC 0x4D) into the BLE stack:
 * pushes the full item set to the peer after a LOCAL edit + on (re)connect, and
 * routes inbound NOTES_SYNC chunks into the LWW merge ([NoteSync]). BLE-only —
 * the merge + connect-push keep both sides eventually consistent across drops.
 * Extension fn on [VortexStack]; call once after the GATT server is up.
 */
internal fun VortexStack.startNotesSync() {
    NoteStore.init(ctx) // load even when the UI isn't open
    NoteSync.init(scope)
    // Send one chunk to every trusted peer over BLE notify.
    NoteSync.sendChunk = { chunk ->
        gattServer?.let { server ->
            for (peer in peerStore.list()) {
                server.sendNotesSyncEncrypted(peer.peerStaticPub, chunk)
            }
        }
    }
    // A local edit (create/edit/check/delete) → debounced full-set push.
    NoteStore.onLocalEdit = { NoteSync.markDirty() }
    // Laptop→phone NOTES_SYNC chunk → reassemble + merge (+ reply if we hold more).
    gattServer?.onNotesSyncReceived = { _, chunk -> NoteSync.onInbound(chunk) }
    // Re-arm due-date reminders whenever the set changes (local edit or sync
    // merge) and once on start (covers re-arm after a reboot). The StateFlow
    // emits its current value immediately on collect.
    scope.launch {
        NoteStore.notes.collect { NoteReminderScheduler.reschedule(ctx, NoteStore.snapshot()) }
    }
}
