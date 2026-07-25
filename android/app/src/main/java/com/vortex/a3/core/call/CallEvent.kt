package com.vortex.a3.core.call

import org.json.JSONObject

/**
 * A phone-call event mirrored to the laptop (continuity-style call
 * banner + in-call pill). One in-flight call at a time; the same [id] threads
 * the ringing → active → ended updates so the laptop updates one banner/pill in
 * place.
 *
 * Tiny by design — fits a single BLE notify. Mirrors Rust
 * `core::call_event::CallEvent`. Carried in a [com.vortex.a3.core.ble.FrameType.CALL]
 * frame, routed entirely separately from the audio-handoff AUDIO_OP path.
 */
data class CallEvent(
    /** Stable identity for this call across its phase updates. */
    val id: String,
    /** "ringing" (incoming, unanswered), "active" (connected), "ended". */
    val phase: String,
    /** Resolved contact display name, or "" when unknown / no permission. */
    val name: String = "",
    /** Caller number (E.164 or raw), or "" when withheld / no permission. */
    val number: String = "",
    /** Epoch-millis the call connected (phase=active) so the laptop can tick a
     *  local duration timer without per-second BLE traffic. 0 until answered. */
    val startedAt: Long = 0L,
    /** True for an outgoing call (laptop UI can label it differently). */
    val outgoing: Boolean = false,
    /** True once an OUTGOING call actually connected (callee answered) — set
     *  from the dialer's live-notification chronometer, since TelephonyCallback
     *  can't tell dialing from connected. Until then the laptop shows "Calling…"
     *  with no timer; once connected it ticks from [startedAt]. */
    val connected: Boolean = false,
    /** Default dialer package (e.g. "com.google.android.dialer") — the laptop
     *  looks up this app's icon (sent once over BLE) so the call banner/pill
     *  shows the phone's real Phone-app logo instead of a generic glyph. */
    val appId: String = "",
    /** Epoch-millis (phone clock) this event was put on the wire. Lets the
     *  laptop anchor the in-call timer to ITS OWN clock —
     *  `started_at_laptop = started_at + (recv_now - sent_at)` — so the
     *  duration matches the phone to the second regardless of clock skew
     *  between the two devices (the only residual error is one-way latency,
     *  ~tens of ms). Re-stamped on each send (BLE frame + every AppState
     *  heartbeat). 0 = not stamped (laptop falls back to raw started_at). */
    val sentAt: Long = 0L,
    /** Live in-call audio state (read at send time) so the laptop pill's card
     *  shows the right toggles: [muted] → Mute/Unmute, [speaker] → Speaker
     *  on/off, [hasEarbuds] → hide Speaker entirely (no point routing to the
     *  loudspeaker when wireless buds are on). */
    val muted: Boolean = false,
    val speaker: Boolean = false,
    val hasEarbuds: Boolean = false,
) {
    fun toJsonBytes(): ByteArray {
        val o = JSONObject()
        o.put("id", id)
        o.put("phase", phase)
        if (name.isNotEmpty()) o.put("name", name)
        if (number.isNotEmpty()) o.put("number", number)
        if (startedAt > 0) o.put("started_at", startedAt)
        if (outgoing) o.put("outgoing", true)
        if (connected) o.put("connected", true)
        if (appId.isNotEmpty()) o.put("app_id", appId)
        if (sentAt > 0) o.put("sent_at", sentAt)
        if (muted) o.put("muted", true)
        if (speaker) o.put("speaker", true)
        if (hasEarbuds) o.put("has_earbuds", true)
        return o.toString().toByteArray(Charsets.UTF_8)
    }

    companion object {
        const val PHASE_RINGING = "ringing"
        const val PHASE_ACTIVE = "active"
        const val PHASE_ENDED = "ended"

        fun fromJsonBytes(bytes: ByteArray): CallEvent? = try {
            val o = JSONObject(String(bytes, Charsets.UTF_8))
            val id = o.optString("id", "")
            val phase = o.optString("phase", "")
            if (id.isEmpty() || phase.isEmpty()) null
            else CallEvent(
                id = id,
                phase = phase,
                name = o.optString("name", ""),
                number = o.optString("number", ""),
                startedAt = o.optLong("started_at", 0L),
                outgoing = o.optBoolean("outgoing", false),
                connected = o.optBoolean("connected", false),
                appId = o.optString("app_id", ""),
                sentAt = o.optLong("sent_at", 0L),
                muted = o.optBoolean("muted", false),
                speaker = o.optBoolean("speaker", false),
                hasEarbuds = o.optBoolean("has_earbuds", false),
            )
        } catch (_: Throwable) {
            null
        }
    }
}

/**
 * A laptop→phone call-control command (the counterpart carried in a
 * [com.vortex.a3.core.ble.FrameType.CALL_CONTROL] frame). [id] targets the call
 * the command applies to; [action] is one of the [Action] constants.
 */
data class CallControl(
    val id: String,
    val action: String,
    /** Optional payload (e.g. the SMS body for [Action.SMS_REJECT]). */
    val arg: String = "",
    /** Monotonic per-command nonce set by the laptop. The phone dedups the
     *  SAME command arriving twice (e.g. the BLE CALL_CONTROL frame AND the
     *  AppState `call_control` fallback) by ignoring `seq <= lastSeq`, while
     *  still letting a repeated action (mute → unmute → mute) through since
     *  each gets a fresh seq. 0 = unsequenced (always handled). */
    val seq: Long = 0L,
) {
    fun toJsonBytes(): ByteArray {
        val o = JSONObject()
        o.put("id", id)
        o.put("action", action)
        if (arg.isNotEmpty()) o.put("arg", arg)
        if (seq > 0) o.put("seq", seq)
        return o.toString().toByteArray(Charsets.UTF_8)
    }

    object Action {
        const val ACCEPT = "accept"
        const val DECLINE = "decline"
        const val END = "end"
        const val MUTE = "mute"
        const val UNMUTE = "unmute"
        const val SPEAKER_ON = "speaker_on"
        const val SPEAKER_OFF = "speaker_off"
        const val SILENCE = "silence"
        const val SMS_REJECT = "sms_reject"

        /** Place a NEW outgoing call from the laptop: [CallControl.id] is empty
         *  and [CallControl.arg] holds the phone number to dial. */
        const val ORIGINATE_CALL = "originate_call"

        /** Send an SMS from the laptop: [CallControl.id] is empty and
         *  [CallControl.arg] is a JSON object `{"to":"<number>","body":"<text>"}`. */
        const val SEND_SMS = "send_sms"

        /** Mark a conversation read on the phone (the laptop opened it):
         *  [CallControl.arg] is `{"thread":<id>,"address":"<number>"}`. Requires
         *  the phone to be able to write the SMS provider (default-SMS-app). */
        const val MARK_READ = "mark_read"

        /** Load one page of a conversation's history for the laptop's infinite
         *  scroll: [CallControl.arg] is
         *  `{"thread":<id>,"address":"<number>","offset":<n>,"limit":<n>}`. The
         *  phone reads that thread (newest-first, skip offset) and replies with
         *  SMS_THREAD frames (merged into the open thread). */
        const val LOAD_THREAD = "load_thread"

        /** Transport controls for the PHONE's media, driven from the laptop's
         *  now-playing pill. Like [LOAD_THREAD] these aren't call actions —
         *  they ride the same command channel. [CallControl.id] is empty;
         *  [CallControl.arg] is the target player's package name (the pill's
         *  app_id) so multi-player phones control the right session. */
        const val MEDIA_PLAY_PAUSE = "media_play_pause"
        const val MEDIA_NEXT = "media_next"
        const val MEDIA_PREV = "media_prev"
    }

    companion object {
        fun fromJsonBytes(bytes: ByteArray): CallControl? = try {
            val o = JSONObject(String(bytes, Charsets.UTF_8))
            val id = o.optString("id", "")
            val action = o.optString("action", "")
            // id may be empty for ORIGINATE_CALL (a brand-new call has no id yet);
            // only the action is mandatory.
            if (action.isEmpty()) null
            else CallControl(
                id = id,
                action = action,
                arg = o.optString("arg", ""),
                seq = o.optLong("seq", 0L),
            )
        } catch (_: Throwable) {
            null
        }
    }
}
