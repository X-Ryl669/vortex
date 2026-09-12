package com.vortex.a3.core

import android.content.BroadcastReceiver
import android.content.Context
import android.content.IntentFilter
import android.os.Build

/**
 * Register a receiver for one of the app's OWN actions so that only the app
 * can reach it.
 *
 * `RECEIVER_NOT_EXPORTED` says exactly this, and is what every one of these
 * call sites passes — from Android 13. The flag does not exist below that, and
 * `minSdk` is 29, so on Android 10 through 12 the same receivers fell through
 * to the two-argument `registerReceiver`, which leaves a receiver IMPLICITLY
 * EXPORTED. Any app on the phone, holding no permission at all, could then
 * broadcast to them. `setPackage()` on the sending side does not help: it
 * constrains where an intent is delivered, not who may send one, and an
 * attacker sets it themselves.
 *
 * What that was worth depended on the action. Stopping a Find-My ring or
 * pressing pause on the laptop's music is a nuisance. The file-consent
 * decision is not: its request ids count up from a fixed 7000, so a handful of
 * broadcasts could accept an incoming transfer the user was never shown —
 * defeating the one prompt that feature exists to put in front of them.
 *
 * [INTERNAL_BROADCAST] is declared `signature`, so it is held only by a build
 * signed with the same key. The senders are all PendingIntents this app
 * created, and a PendingIntent is sent with its creator's identity, so they
 * satisfy it without any change of their own.
 */
const val INTERNAL_BROADCAST: String = "com.vortex.a3.permission.INTERNAL_BROADCAST"

fun Context.registerInternalReceiver(receiver: BroadcastReceiver, filter: IntentFilter) {
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
    } else {
        // Same guarantee, expressed the only way this API version can: a
        // permission no other app can hold.
        @Suppress("UnspecifiedRegisterReceiverFlag")
        registerReceiver(receiver, filter, INTERNAL_BROADCAST, null)
    }
}
