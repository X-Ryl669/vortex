package com.vortex.a3.service

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.util.Log
import com.vortex.a3.core.storage.EncryptedPrefsPeerStore

/**
 * Auto-start the Vortex foreground service after device boot if any peer
 * is already trusted. This mirrors the "Telegram-style" expectation:
 * after a reboot, the app is reachable to its paired peers without the
 * user having to relaunch it.
 *
 * No-op if trust store is empty — pre-trust advertising is a privacy-
 * sensitive action that must be user-initiated.
 *
 * On MIUI / OEM-customised Android, an Autostart permission must be
 * granted manually by the user from the Security Center; this receiver
 * cannot bypass that.
 */
class BootReceiver : BroadcastReceiver() {

    override fun onReceive(context: Context, intent: Intent) {
        val action = intent.action
        Log.i(TAG, "received $action")
        if (action != Intent.ACTION_BOOT_COMPLETED &&
            action != Intent.ACTION_LOCKED_BOOT_COMPLETED &&
            action != "android.intent.action.QUICKBOOT_POWERON" &&
            action != "com.htc.intent.action.QUICKBOOT_POWERON"
        ) {
            return
        }

        val peerStore = EncryptedPrefsPeerStore(context.applicationContext)
        val peerCount = peerStore.list().size
        if (peerCount == 0) {
            Log.i(TAG, "no trusted peers; skipping autostart")
            return
        }
        Log.i(TAG, "autostarting VortexService (have $peerCount trusted peer(s))")
        VortexService.start(context.applicationContext)
    }

    companion object {
        private const val TAG = "VortexBoot"
    }
}
