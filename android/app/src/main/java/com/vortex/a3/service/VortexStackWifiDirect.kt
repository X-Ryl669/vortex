package com.vortex.a3.service

import android.content.Context
import android.os.Build
import android.util.Log
import kotlinx.coroutines.launch

/**
 * Wi-Fi Direct (direct-link fast pull) — split out of [VortexStack]. A big
 * shared file brings up a P2P group owner on the phone and tells the laptop to
 * join for an ~20 MB/s direct link; the group is torn down after an idle window.
 * Also wires a debug-only broadcast hook for manual GO validation.
 */

/** Create the Wi-Fi Direct GO (idempotent) and, once it's up, tell the laptop
 *  to join; tear it down after an idle window (reset on each big file). */
internal fun VortexStack.maybeStartWifiDirect() {
    com.vortex.a3.core.lan.WifiDirect.start(ctx) {
        val o = org.json.JSONObject()
        o.put("ssid", com.vortex.a3.core.lan.WifiDirect.SSID)
        o.put("pass", com.vortex.a3.core.lan.WifiDirect.PASS)
        val offer = o.toString().toByteArray(Charsets.UTF_8)
        for (peer in peerStore.list()) {
            gattServer?.sendWifiDirectOfferEncrypted(peer.peerStaticPub, offer)
        }
        Log.i(VortexStack.TAG, "wifi-direct: GO ready → offer sent to laptop")
    }
    wifiDirectTeardownJob?.cancel()
    wifiDirectTeardownJob = scope.launch {
        kotlinx.coroutines.delay(60_000)
        com.vortex.a3.core.lan.WifiDirect.stop()
        Log.i(VortexStack.TAG, "wifi-direct: GO torn down (idle)")
    }
}

/**
 * Debug-only Wi-Fi Direct validation hook:
 *   adb shell am broadcast -a com.vortex.a3.WIFI_DIRECT --es mode on
 *   adb shell am broadcast -a com.vortex.a3.WIFI_DIRECT --es mode off
 * `on` brings up the GO (logs SSID/passphrase/IP); the laptop joins manually
 * to measure the direct-link speed. EXPORTED so adb can reach it (debug only).
 */
internal fun VortexStack.registerWifiDirectReceiver() {
    if (!com.vortex.a3.BuildConfig.DEBUG) return
    val receiver = object : android.content.BroadcastReceiver() {
        override fun onReceive(c: android.content.Context?, intent: android.content.Intent?) {
            when (intent?.getStringExtra("mode")?.lowercase()) {
                "on" -> {
                    Log.i(VortexStack.TAG, "WIFI_DIRECT broadcast → on")
                    com.vortex.a3.core.lan.WifiDirect.start(ctx)
                }
                "off" -> {
                    Log.i(VortexStack.TAG, "WIFI_DIRECT broadcast → off")
                    com.vortex.a3.core.lan.WifiDirect.stop()
                }
                else -> Log.w(VortexStack.TAG, "WIFI_DIRECT: --es mode on|off required")
            }
        }
    }
    val filter = android.content.IntentFilter("com.vortex.a3.WIFI_DIRECT")
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
        service.registerReceiver(receiver, filter, Context.RECEIVER_EXPORTED)
    } else {
        @Suppress("UnspecifiedRegisterReceiverFlag")
        service.registerReceiver(receiver, filter)
    }
    wifiDirectReceiver = receiver
}
