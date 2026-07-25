package com.vortex.a3.core.contacts

import android.content.Context
import android.content.pm.PackageManager
import android.database.ContentObserver
import android.os.Handler
import android.os.Looper
import android.provider.ContactsContract
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.Job
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.launch

/**
 * Reads the phone's contacts (name + numbers) and emits the full list whenever
 * it changes, so the laptop companion's Contacts page can mirror it. One query
 * over `Phone.CONTENT_URI` (one row per number) grouped by contact id; a
 * `ContentObserver` re-emits on any add/edit/delete. READ_CONTACTS gated.
 */
class ContactsProvider(
    private val context: Context,
    private val onContacts: (List<Contact>) -> Unit,
) {
    private val tag = "ContactsProvider"
    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.IO)
    private var observer: ContentObserver? = null

    companion object {
        /** Quiet window before re-reading + re-sending after an onChange burst. */
        private const val EMIT_DEBOUNCE_MS = 2_000L
    }

    fun start() {
        if (!hasPermission()) {
            Log.i(tag, "READ_CONTACTS missing; contacts mirror disabled")
            return
        }
        scope.launch { emitSnapshot() }
        val obs = object : ContentObserver(Handler(Looper.getMainLooper())) {
            override fun onChange(selfChange: Boolean) {
                scheduleEmit()
            }
        }
        try {
            context.contentResolver.registerContentObserver(
                ContactsContract.Contacts.CONTENT_URI, true, obs,
            )
            observer = obs
        } catch (e: Exception) {
            Log.w(tag, "registerContentObserver: ${e.message}")
        }
    }

    /** Battery: each emit ships the full list as a BLE chunk burst, so collapse
     *  observer bursts (a contact import fires onChange per row) into one
     *  re-read after a quiet window. */
    private fun scheduleEmit() {
        pendingEmit?.cancel()
        pendingEmit = scope.launch {
            kotlinx.coroutines.delay(EMIT_DEBOUNCE_MS)
            emitSnapshot()
        }
    }

    private var pendingEmit: Job? = null

    fun stop() {
        observer?.let {
            try { context.contentResolver.unregisterContentObserver(it) } catch (_: Exception) {}
        }
        observer = null
        scope.coroutineContext[Job]?.cancel()
    }

    /** Re-read + emit now (e.g. after the user grants READ_CONTACTS post-launch). */
    fun refresh() {
        if (hasPermission()) scope.launch { emitSnapshot() }
    }

    private fun emitSnapshot() {
        try {
            onContacts(readContacts())
        } catch (e: Exception) {
            Log.w(tag, "emitSnapshot: ${e.message}")
        }
    }

    private fun readContacts(): List<Contact> {
        // Preserve insertion order (DISPLAY_NAME ASC) and group numbers per id.
        val byId = LinkedHashMap<String, Pair<String, MutableList<String>>>()
        val cols = arrayOf(
            ContactsContract.CommonDataKinds.Phone.CONTACT_ID,
            ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME,
            ContactsContract.CommonDataKinds.Phone.NUMBER,
        )
        context.contentResolver.query(
            ContactsContract.CommonDataKinds.Phone.CONTENT_URI,
            cols,
            null,
            null,
            ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME + " COLLATE NOCASE ASC",
        )?.use { c ->
            val idIdx = c.getColumnIndex(ContactsContract.CommonDataKinds.Phone.CONTACT_ID)
            val nameIdx = c.getColumnIndex(ContactsContract.CommonDataKinds.Phone.DISPLAY_NAME)
            val numIdx = c.getColumnIndex(ContactsContract.CommonDataKinds.Phone.NUMBER)
            if (idIdx < 0 || numIdx < 0) return emptyList()
            while (c.moveToNext()) {
                val id = c.getString(idIdx) ?: continue
                val name = (if (nameIdx >= 0) c.getString(nameIdx) else null)?.trim().orEmpty()
                val num = c.getString(numIdx)?.trim().orEmpty()
                if (num.isEmpty()) continue
                val entry = byId.getOrPut(id) { name to mutableListOf() }
                if (num !in entry.second) entry.second.add(num)
            }
        }
        return byId.map { (id, v) ->
            Contact(
                id = id,
                name = v.first.ifEmpty { v.second.firstOrNull().orEmpty() },
                numbers = v.second,
            )
        }
    }

    private fun hasPermission(): Boolean =
        ContextCompat.checkSelfPermission(
            context,
            android.Manifest.permission.READ_CONTACTS,
        ) == PackageManager.PERMISSION_GRANTED
}
