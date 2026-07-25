package com.vortex.a3.core.earbuds

import android.Manifest
import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.bluetooth.BluetoothManager
import android.bluetooth.BluetoothProfile
import android.content.Context
import android.content.pm.PackageManager
import android.util.Log
import androidx.core.content.ContextCompat
import com.vortex.a3.core.appstate.EarbudsInfo

/**
 * Resolves the earbud card's live state.
 *
 * Saved-entry only: the card represents whichever device the user
 * explicitly picked through the in-app modal. We no longer auto-
 * detect the first connected audio peripheral — that turned
 * "Remove from Vortex" into a no-op, since the system bond stayed
 * and the next poll would re-catch the same buds. Returning null
 * when nothing is saved lets the home screen render the
 * "+ Add earbuds" placeholder cleanly.
 *
 * Battery percentage uses the hidden `BluetoothDevice.getBatteryLevel`
 * (publicly available via reflection; same value Settings →
 * Connected devices shows). Returns null when the OS has no battery
 * reading yet.
 */
object EarbudsDetector {

    private const val TAG = "VortexEarbuds"

    fun readConnectedEarbuds(context: Context): EarbudsInfo? {
        if (!hasBluetoothConnectPermission(context)) return null
        val bm = context.applicationContext.getSystemService(BluetoothManager::class.java)
            ?: return null
        val adapter: BluetoothAdapter = bm.adapter ?: return null
        if (!adapter.isEnabled) return null

        val saved = EarbudsStore.load(context) ?: return null
        return readSavedRow(bm, adapter, saved)
    }

    private fun readSavedRow(
        bm: BluetoothManager,
        adapter: BluetoothAdapter,
        saved: SavedEarbuds,
    ): EarbudsInfo {
        // Try the bonded list first — that's the canonical live `BluetoothDevice`
        // (battery & connection-state accessors work on it).
        val bonded = try {
            adapter.bondedDevices ?: emptySet()
        } catch (_: SecurityException) {
            emptySet()
        }
        val match = bonded.firstOrNull {
            it.address.equals(saved.address, ignoreCase = true)
        }
        val displayName = match?.let { safeName(it) }?.takeIf { it.isNotBlank() }
            ?: saved.name
        if (match == null) {
            // Not in bonded set — buds were forgotten in system Settings.
            // Still keep the card visible; the user can re-pick when they
            // re-bond.
            return EarbudsInfo(name = displayName, address = saved.address, battery = null, connected = false)
        }
        val connected = isConnected(bm, match)
        val battery = if (connected) readBatteryLevel(match) else null
        return EarbudsInfo(name = displayName, address = saved.address, battery = battery, connected = connected)
    }

    private fun safeName(device: BluetoothDevice): String? = try {
        device.name
    } catch (_: SecurityException) {
        null
    }

    private fun isConnected(bm: BluetoothManager, device: BluetoothDevice): Boolean {
        for (profile in intArrayOf(BluetoothProfile.HEADSET, BluetoothProfile.GATT)) {
            val state = try {
                bm.getConnectionState(device, profile)
            } catch (_: IllegalArgumentException) {
                continue
            } catch (_: SecurityException) {
                continue
            }
            if (state == BluetoothProfile.STATE_CONNECTED) return true
        }
        return try {
            val m = BluetoothDevice::class.java.getMethod("isConnected")
            (m.invoke(device) as? Boolean) ?: false
        } catch (_: Throwable) {
            false
        }
    }

    private fun readBatteryLevel(device: BluetoothDevice): Int? {
        return try {
            val method = BluetoothDevice::class.java.getMethod("getBatteryLevel")
            val raw = method.invoke(device) as? Int ?: return null
            if (raw < 0 || raw > 100) null else raw
        } catch (_: Throwable) {
            null
        }
    }

    private fun hasBluetoothConnectPermission(context: Context): Boolean {
        return if (android.os.Build.VERSION.SDK_INT >= android.os.Build.VERSION_CODES.S) {
            ContextCompat.checkSelfPermission(
                context,
                Manifest.permission.BLUETOOTH_CONNECT,
            ) == PackageManager.PERMISSION_GRANTED
        } else {
            true
        }
    }
}
