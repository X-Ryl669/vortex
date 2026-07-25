package com.vortex.a3.service

import android.bluetooth.BluetoothAdapter
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.os.Build
import android.os.BatteryManager
import android.util.Log

/**
 * The two infrastructure broadcast receivers VortexService relies on,
 * pulled out so the service file stays focused on the stack itself:
 *
 *  - **BT adapter state** — Android tears down all BLE advertising + the
 *    GATT server when the adapter turns off and never restores them, so we
 *    rebuild on STATE_ON (via [onBluetoothReenabled]).
 *  - **Battery** — edge-detected: a charging flip pushes instantly, a level
 *    change only on a ≥2-point delta, so a slow drain doesn't spam the peer
 *    (via [onBatteryChanged]).
 *
 * The service supplies the reactions as callbacks; this class owns the
 * receiver instances, the battery edge-detect state, and register/unregister
 * lifecycle. (The debug-only FAKE_CALL receiver stays in the service — it is
 * wired directly to the call-flow orchestrator.)
 */
class VortexReceivers(
    private val context: Context,
    private val onBluetoothReenabled: () -> Unit,
    private val onBatteryChanged: () -> Unit,
) {
    private var btStateReceiver: BroadcastReceiver? = null
    private var batteryReceiver: BroadcastReceiver? = null
    @Volatile private var lastBattCharging: Boolean? = null
    @Volatile private var lastBattLevel: Int = -1

    fun register() {
        registerBtState()
        registerBattery()
    }

    fun unregister() {
        btStateReceiver?.let { safeUnregister(it) }
        batteryReceiver?.let { safeUnregister(it) }
        btStateReceiver = null
        batteryReceiver = null
    }

    private fun registerBtState() {
        if (btStateReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context?, intent: Intent?) {
                if (intent?.action != BluetoothAdapter.ACTION_STATE_CHANGED) return
                when (intent.getIntExtra(BluetoothAdapter.EXTRA_STATE, BluetoothAdapter.ERROR)) {
                    BluetoothAdapter.STATE_ON -> onBluetoothReenabled()
                    BluetoothAdapter.STATE_OFF ->
                        Log.i(TAG, "Bluetooth turned off — BLE down until re-enabled")
                }
            }
        }
        registerReceiverCompat(receiver, IntentFilter(BluetoothAdapter.ACTION_STATE_CHANGED))
        btStateReceiver = receiver
    }

    /**
     * Event-driven battery push. ACTION_BATTERY_CHANGED fires on every
     * charging flip (plug/unplug) and level tick. We edge-detect: a
     * charging-state flip fires [onBatteryChanged] instantly; a level change
     * only when it moves ≥2 points so a slow drain doesn't spam the peer.
     */
    private fun registerBattery() {
        if (batteryReceiver != null) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(ctx: Context?, intent: Intent?) {
                if (intent?.action != Intent.ACTION_BATTERY_CHANGED) return
                val status = intent.getIntExtra(BatteryManager.EXTRA_STATUS, -1)
                val charging = status == BatteryManager.BATTERY_STATUS_CHARGING ||
                    status == BatteryManager.BATTERY_STATUS_FULL
                val rawLevel = intent.getIntExtra(BatteryManager.EXTRA_LEVEL, -1)
                val scale = intent.getIntExtra(BatteryManager.EXTRA_SCALE, -1)
                val level = if (rawLevel >= 0 && scale > 0) rawLevel * 100 / scale else -1

                val chargingFlipped = lastBattCharging != null && lastBattCharging != charging
                val levelMoved = lastBattLevel >= 0 && level >= 0 &&
                    kotlin.math.abs(level - lastBattLevel) >= 2
                val firstSeen = lastBattCharging == null
                lastBattCharging = charging
                lastBattLevel = level
                // Skip the very first sticky broadcast (just seeds state).
                if (firstSeen) return
                if (chargingFlipped || levelMoved) {
                    Log.i(TAG, "battery event (charging=$charging level=$level) → push")
                    onBatteryChanged()
                }
            }
        }
        registerReceiverCompat(receiver, IntentFilter(Intent.ACTION_BATTERY_CHANGED))
        batteryReceiver = receiver
    }

    private fun registerReceiverCompat(receiver: BroadcastReceiver, filter: IntentFilter) {
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            context.registerReceiver(receiver, filter, Context.RECEIVER_NOT_EXPORTED)
        } else {
            @Suppress("UnspecifiedRegisterReceiverFlag")
            context.registerReceiver(receiver, filter)
        }
    }

    private fun safeUnregister(receiver: BroadcastReceiver) {
        try { context.unregisterReceiver(receiver) } catch (_: Exception) {}
    }

    companion object {
        private const val TAG = "VortexReceivers"
    }
}
