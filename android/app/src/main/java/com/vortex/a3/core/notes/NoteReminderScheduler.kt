package com.vortex.a3.core.notes

import android.app.AlarmManager
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.os.Build
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat

/**
 * Schedules a local alarm for each todo with a future due-date and posts a
 * reminder notification when it fires. Independent of the laptop — each device
 * arms its own alarms from the synced [Note.dueAt] (the reminder fires on
 * all devices). Re-armed whenever the note set changes (edit / sync merge) and
 * on boot; an alarm is cancelled when its todo is done / deleted / re-dated.
 */
object NoteReminderScheduler {
    const val ACTION_FIRE = "com.vortex.a3.NOTE_REMINDER"
    private const val CHANNEL_ID = "vortex_note_reminders"
    @Volatile private var channelReady = false

    /** Re-arm exact alarms for the whole set (incl. tombstones, so a deleted /
     *  done todo's stale alarm is cancelled). Idempotent — safe to call often. */
    fun reschedule(context: Context, items: List<Note>) {
        val am = context.getSystemService(AlarmManager::class.java) ?: return
        val now = System.currentTimeMillis()
        for (n in items) {
            if (n.kind != "todo") continue
            val pi = firePendingIntent(context, n)
            am.cancel(pi) // clear any prior alarm for this id
            if (!n.done && !n.deleted && n.dueAt > now) {
                try {
                    am.setExactAndAllowWhileIdle(AlarmManager.RTC_WAKEUP, n.dueAt, pi)
                } catch (_: SecurityException) {
                    am.set(AlarmManager.RTC_WAKEUP, n.dueAt, pi) // no exact-alarm grant → inexact
                }
            }
        }
    }

    private fun firePendingIntent(context: Context, n: Note): PendingIntent {
        val i = Intent(context, NoteReminderReceiver::class.java).apply {
            action = ACTION_FIRE
            putExtra("id", n.id)
            putExtra("title", n.title)
        }
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE
        return PendingIntent.getBroadcast(context, n.id.hashCode(), i, flags)
    }

    /** Post the reminder notification (called from the receiver when an alarm
     *  fires). */
    fun fireNotification(context: Context, id: String, title: String) {
        ensureChannel(context)
        val n = NotificationCompat.Builder(context, CHANNEL_ID)
            .setSmallIcon(android.R.drawable.ic_popup_reminder)
            .setContentTitle(title.ifBlank { "Reminder" })
            .setContentText(if (title.isBlank()) "Todo" else "Reminder")
            .setAutoCancel(true)
            .setCategory(NotificationCompat.CATEGORY_REMINDER)
            .setPriority(NotificationCompat.PRIORITY_HIGH)
            .build()
        try {
            NotificationManagerCompat.from(context).notify(id.hashCode(), n)
        } catch (_: SecurityException) {
            // POST_NOTIFICATIONS not granted — nothing to show.
        }
    }

    private fun ensureChannel(context: Context) {
        if (channelReady) return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val ch = NotificationChannel(
                CHANNEL_ID,
                "Note reminders",
                NotificationManager.IMPORTANCE_HIGH,
            ).apply { description = "Todo due-date reminders" }
            context.getSystemService(NotificationManager::class.java)
                ?.createNotificationChannel(ch)
        }
        channelReady = true
    }
}
