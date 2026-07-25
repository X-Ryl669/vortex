package com.vortex.a3.core.call

import android.content.Context
import android.content.Intent
import android.content.pm.PackageManager
import android.media.AudioManager
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.telecom.TelecomManager
import android.util.Log
import androidx.core.content.ContextCompat

/**
 * Acts on the current phone call on behalf of the laptop's call banner: the
 * laptop sends a [CallControl] over BLE (Accept / Decline / End / Mute / …) and
 * this maps it to the platform APIs. Answer/end go through [TelecomManager]
 * (needs the runtime-grantable ANSWER_PHONE_CALLS permission, API 26+); mute /
 * speaker go through [AudioManager] (no special permission). Every action is
 * best-effort and never throws — a missing permission just logs + no-ops.
 *
 * Wholly separate from [CallFlowOrchestrator] (which only observes call state
 * for the audio handoff); this is the laptop-driven control side.
 */
class CallController(private val context: Context) {

    private val tag = "CallController"

    fun handle(ctrl: CallControl) {
        Log.i(tag, "call-control action=${ctrl.action}")
        when (ctrl.action) {
            CallControl.Action.ACCEPT -> acceptCall()
            CallControl.Action.DECLINE, CallControl.Action.END -> endCall()
            CallControl.Action.MUTE -> setMute(true)
            CallControl.Action.UNMUTE -> setMute(false)
            CallControl.Action.SILENCE -> silenceRinger()
            CallControl.Action.SPEAKER_ON -> setSpeaker(true)
            CallControl.Action.SPEAKER_OFF -> setSpeaker(false)
            CallControl.Action.ORIGINATE_CALL -> placeCall(ctrl.arg)
            CallControl.Action.SEND_SMS -> sendSms(ctrl.arg)
            CallControl.Action.MARK_READ -> markRead(ctrl.arg)
            CallControl.Action.MEDIA_PLAY_PAUSE,
            CallControl.Action.MEDIA_NEXT,
            CallControl.Action.MEDIA_PREV,
            -> mediaAction(ctrl.action, ctrl.arg)
            else -> Log.w(tag, "unhandled call action: ${ctrl.action}")
        }
    }

    /** Transport control for the phone's media, driven from the laptop's
     *  now-playing pill. Targets the session of package [pkg] (the pill's
     *  app_id), falling back to whichever session is playing, then to a
     *  global media-key event. Best-effort like everything here. */
    private fun mediaAction(action: String, pkg: String) {
        val controller = try {
            val msm = context.getSystemService(Context.MEDIA_SESSION_SERVICE)
                as? android.media.session.MediaSessionManager
            val sessions = msm?.getActiveSessions(
                android.content.ComponentName(
                    context,
                    com.vortex.a3.core.media.MediaNotificationListenerService::class.java,
                ),
            )?.filter { it.packageName != context.packageName }.orEmpty()
            sessions.firstOrNull { it.packageName == pkg }
                ?: sessions.firstOrNull {
                    it.playbackState?.state ==
                        android.media.session.PlaybackState.STATE_PLAYING
                }
                ?: sessions.firstOrNull()
        } catch (e: Exception) {
            Log.w(tag, "media session lookup failed: ${e.message}")
            null
        }
        if (controller == null) {
            mediaKeyFallback(action)
        } else {
            try {
                val tc = controller.transportControls
                when (action) {
                    CallControl.Action.MEDIA_PLAY_PAUSE -> {
                        val playing = controller.playbackState?.state ==
                            android.media.session.PlaybackState.STATE_PLAYING
                        if (playing) tc.pause() else tc.play()
                    }
                    CallControl.Action.MEDIA_NEXT -> tc.skipToNext()
                    CallControl.Action.MEDIA_PREV -> tc.skipToPrevious()
                }
            } catch (e: Exception) {
                Log.w(tag, "media transport failed: ${e.message}")
            }
        }
        // Push the fresh ⏸/▶ state back to the laptop promptly — the app may
        // not repost its notification, but the session state is already new.
        android.os.Handler(android.os.Looper.getMainLooper()).postDelayed(
            { com.vortex.a3.core.media.MediaNotificationListenerService.rescanMediaPills() },
            400,
        )
    }

    /** No addressable session (some players only expose media-button input):
     *  synthesize the matching hardware media key. */
    private fun mediaKeyFallback(action: String) {
        val code = when (action) {
            CallControl.Action.MEDIA_NEXT -> android.view.KeyEvent.KEYCODE_MEDIA_NEXT
            CallControl.Action.MEDIA_PREV -> android.view.KeyEvent.KEYCODE_MEDIA_PREVIOUS
            else -> android.view.KeyEvent.KEYCODE_MEDIA_PLAY_PAUSE
        }
        try {
            val am = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
            am.dispatchMediaKeyEvent(android.view.KeyEvent(android.view.KeyEvent.ACTION_DOWN, code))
            am.dispatchMediaKeyEvent(android.view.KeyEvent(android.view.KeyEvent.ACTION_UP, code))
        } catch (e: Exception) {
            Log.w(tag, "media key fallback failed: ${e.message}")
        }
    }

    /** Mark a conversation read on the phone (the laptop opened it). [argJson] =
     *  `{"thread":<id>,"address":"<number>"}`. Writing the SMS provider is
     *  restricted to the default SMS app on Android 4.4+, so this best-effort
     *  update may no-op (SecurityException) — the laptop still shows it read. */
    private fun markRead(argJson: String) {
        val thread: Long
        val address: String
        try {
            val o = org.json.JSONObject(argJson)
            thread = o.optLong("thread", 0L)
            address = o.optString("address", "").trim()
        } catch (e: Exception) {
            Log.w(tag, "mark_read: bad arg: ${e.message}")
            return
        }
        val values = android.content.ContentValues().apply {
            put("read", 1)
            put("seen", 1)
        }
        try {
            val rows = when {
                thread > 0L -> context.contentResolver.update(
                    android.provider.Telephony.Sms.CONTENT_URI, values,
                    "thread_id=? AND read=0", arrayOf(thread.toString()),
                )
                address.isNotEmpty() -> context.contentResolver.update(
                    android.provider.Telephony.Sms.CONTENT_URI, values,
                    "address=? AND read=0", arrayOf(address),
                )
                else -> 0
            }
            Log.i(tag, "mark_read: updated $rows row(s)")
        } catch (e: Exception) {
            Log.w(tag, "mark_read failed (default-SMS-app required to write): ${e.message}")
        }
    }

    private fun hasSendSmsPerm(): Boolean =
        ContextCompat.checkSelfPermission(
            context,
            android.Manifest.permission.SEND_SMS,
        ) == PackageManager.PERMISSION_GRANTED

    /** Send an SMS the laptop composed. [argJson] = `{"to":..,"body":..}`. The
     *  body is sensitive — never logged. Auto-sends via SmsManager when SEND_SMS
     *  is available; otherwise opens the SMS app pre-filled (the user taps send),
     *  which needs no permission (MIUI restricts SEND_SMS for sideloaded apps). */
    private fun sendSms(argJson: String) {
        val to: String
        val body: String
        try {
            val o = org.json.JSONObject(argJson)
            to = o.optString("to", "").trim()
            body = o.optString("body", "")
        } catch (e: Exception) {
            Log.w(tag, "send_sms: bad arg: ${e.message}")
            return
        }
        if (to.isEmpty() || body.isEmpty()) {
            Log.w(tag, "send_sms: empty to/body")
            return
        }
        // PRIMARY: SmsManager (auto-send). Attempt even when the runtime flag
        // reads denied — some ROMs gate on the appop; a real block throws.
        try {
            @Suppress("DEPRECATION")
            val sm = context.getSystemService(android.telephony.SmsManager::class.java)
                ?: android.telephony.SmsManager.getDefault()
            val parts = sm.divideMessage(body)
            if (parts.size > 1) {
                sm.sendMultipartTextMessage(to, null, parts, null, null)
            } else {
                sm.sendTextMessage(to, null, body, null, null)
            }
            Log.i(tag, "send_sms: sent via SmsManager (granted=${hasSendSmsPerm()})")
            return
        } catch (e: Exception) {
            Log.w(tag, "send_sms: SmsManager failed (${e.message}); opening SMS app")
        }
        // FALLBACK: open the messaging app pre-filled; user taps send. No perm.
        try {
            val intent = android.content.Intent(
                android.content.Intent.ACTION_SENDTO,
                android.net.Uri.fromParts("smsto", to, null),
            ).putExtra("sms_body", body)
                .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
            Log.i(tag, "send_sms: opened SMS app (no SEND_SMS → user confirms)")
        } catch (e: Exception) {
            Log.w(tag, "send_sms: ACTION_SENDTO failed: ${e.message}")
        }
    }

    private fun hasCallPhonePerm(): Boolean =
        ContextCompat.checkSelfPermission(
            context,
            android.Manifest.permission.CALL_PHONE,
        ) == PackageManager.PERMISSION_GRANTED

    /** Place a new outgoing call to [number] (laptop-initiated dial). The
     *  existing [CallFlowOrchestrator] then observes the outgoing call and the
     *  laptop's in-call pill appears, just like a phone-initiated call. */
    private fun placeCall(number: String) {
        val num = number.trim()
        if (num.isEmpty()) {
            Log.w(tag, "dial: empty number")
            return
        }
        // PRIMARY: place the call directly. We attempt even when
        // checkSelfPermission() reports denied, because some ROMs (MIUI) gate
        // placeCall on the CALL_PHONE *appop* rather than the runtime grant
        // flag — if it really isn't allowed it throws SecurityException and we
        // fall back to the dialer. (hasCallPhonePerm only logs the flag state.)
        Log.i(tag, "dial: CALL_PHONE runtime-granted=${hasCallPhonePerm()}; trying placeCall")
        try {
            @Suppress("MissingPermission")
            telecom()?.placeCall(Uri.fromParts("tel", num, null), Bundle())
            Log.i(tag, "dial: placeCall (auto)")
            return
        } catch (e: SecurityException) {
            Log.w(tag, "dial: placeCall denied (${e.message}); opening dialer")
        } catch (e: Exception) {
            Log.w(tag, "dial: placeCall failed (${e.message}); opening dialer")
        }
        // FALLBACK (CALL_PHONE truly blocked — e.g. MIUI sideload restriction):
        // open the dialer pre-filled with the number; the user taps call. Needs
        // no permission. ACTION_DIAL is always safe to fire.
        try {
            val intent = Intent(Intent.ACTION_DIAL, Uri.fromParts("tel", num, null))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            context.startActivity(intent)
            Log.i(tag, "dial: opened dialer (no CALL_PHONE → user confirms)")
        } catch (e: Exception) {
            Log.w(tag, "dial: ACTION_DIAL failed: ${e.message}")
        }
    }

    private fun telecom(): TelecomManager? =
        context.getSystemService(Context.TELECOM_SERVICE) as? TelecomManager

    private fun hasAnswerPerm(): Boolean =
        ContextCompat.checkSelfPermission(
            context,
            android.Manifest.permission.ANSWER_PHONE_CALLS,
        ) == PackageManager.PERMISSION_GRANTED

    private fun acceptCall() {
        // PRIMARY: fire the dialer's own Answer action via the notification
        // listener — works where acceptRingingCall() is ROM-gated (MIUI).
        if (com.vortex.a3.core.media.MediaNotificationListenerService
                .fireCallAction(wantAnswer = true)
        ) {
            Log.i(tag, "accept: fired dialer Answer action")
            return
        }
        // FALLBACK: TelecomManager (works on stock AOSP with ANSWER_PHONE_CALLS).
        if (!hasAnswerPerm()) {
            Log.w(tag, "accept: no dialer Answer action and ANSWER_PHONE_CALLS not granted")
            return
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.O) {
            Log.w(tag, "accept: needs API 26+")
            return
        }
        try {
            @Suppress("MissingPermission")
            telecom()?.acceptRingingCall()
        } catch (e: Exception) {
            Log.w(tag, "acceptRingingCall failed: ${e.message}")
        }
    }

    private fun endCall() {
        // PRIMARY: fire the dialer's Decline / Hang-up action.
        if (com.vortex.a3.core.media.MediaNotificationListenerService
                .fireCallAction(wantAnswer = false)
        ) {
            Log.i(tag, "end: fired dialer Decline/Hang-up action")
            return
        }
        // FALLBACK: TelecomManager.endCall (API 28+, ANSWER_PHONE_CALLS).
        if (!hasAnswerPerm()) {
            Log.w(tag, "end: no dialer action and ANSWER_PHONE_CALLS not granted")
            return
        }
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.P) {
            Log.w(tag, "end: needs API 28+")
            return
        }
        try {
            @Suppress("MissingPermission")
            telecom()?.endCall()
        } catch (e: Exception) {
            Log.w(tag, "endCall failed: ${e.message}")
        }
    }

    /** Silence the CURRENT incoming call's ringer WITHOUT answering or
     *  declining — the laptop's banner-dismiss ("silence") gesture, like the
     *  phone's side button. Best-effort: TelecomManager.silenceRinger() needs
     *  MODIFY_PHONE_STATE (system), so it usually no-ops on stock/MIUI; the
     *  call keeps ringing and the laptop still shows an answerable notification.
     */
    private fun silenceRinger() {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) {
            Log.w(tag, "silence: needs API 23+")
            return
        }
        try {
            @Suppress("MissingPermission")
            telecom()?.silenceRinger()
            Log.i(tag, "silenceRinger() called")
        } catch (e: Exception) {
            Log.w(tag, "silenceRinger failed (likely no MODIFY_PHONE_STATE): ${e.message}")
        }
    }

    private fun setMute(mute: Boolean) {
        try {
            val am = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
            am.isMicrophoneMute = mute
        } catch (e: Exception) {
            Log.w(tag, "setMute failed: ${e.message}")
        }
    }

    private fun setSpeaker(on: Boolean) {
        try {
            val am = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager ?: return
            @Suppress("DEPRECATION")
            am.isSpeakerphoneOn = on
        } catch (e: Exception) {
            Log.w(tag, "setSpeaker failed: ${e.message}")
        }
    }
}
