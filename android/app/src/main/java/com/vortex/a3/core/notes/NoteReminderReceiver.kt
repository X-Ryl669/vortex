package com.vortex.a3.core.notes

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent

/**
 * Fires a todo's due-date reminder notification, and re-arms all reminders after
 * a reboot (alarms don't survive a restart). Registered in the manifest for the
 * NOTE_REMINDER action + BOOT_COMPLETED.
 */
class NoteReminderReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context?, intent: Intent?) {
        val ctx = context ?: return
        when (intent?.action) {
            NoteReminderScheduler.ACTION_FIRE -> {
                val id = intent.getStringExtra("id") ?: return
                val title = intent.getStringExtra("title").orEmpty()
                NoteReminderScheduler.fireNotification(ctx, id, title)
            }
            Intent.ACTION_BOOT_COMPLETED -> {
                // Alarms are cleared on reboot — re-arm from the persisted store.
                NoteStore.init(ctx)
                NoteReminderScheduler.reschedule(ctx, NoteStore.snapshot())
            }
        }
    }
}
