package com.vortex.a3.core.ble

import android.bluetooth.BluetoothAdapter
import android.bluetooth.BluetoothDevice
import android.util.Log

/**
 * Removes BT bonds (link keys + IRK) that BlueZ/Linux no longer holds.
 *
 * Why this exists: bonding is symmetric, but the two sides store their keys
 * independently. If Linux clears its peer/bond state (peer-store wipe, a
 * failed `device.pair()`, BlueZ cache reset) while this phone keeps its half,
 * the phone ends up with a one-sided bond for the Linux adapter address. The
 * next time Linux initiates SMP, the phone tries to *encrypt with the old
 * keys* instead of re-pairing and the handshake dies with "Authentication
 * Failed" — every time, in ~6 s, regardless of the user accepting the system
 * pair dialog. Android also hides such profile-less LE bonds from the Settings
 * UI, so the user can't clear them by hand.
 *
 * [BluetoothDevice.removeBond] is the fix but it's a hidden API; we call it by
 * reflection, the same approach the app already uses for `BluetoothA2dp`.
 */
object BondCleaner {
    private const val TAG = "VortexBondCleaner"

    /**
     * Drop the bond for [address] if one exists. Returns true when removeBond
     * was invoked and reported success. Best-effort: an unbonded device, a
     * missing method, or a reflection failure is logged and returns false.
     */
    fun removeBond(adapter: BluetoothAdapter, address: String): Boolean {
        return try {
            val device = adapter.getRemoteDevice(address)
            if (device.bondState == BluetoothDevice.BOND_NONE) {
                Log.i(TAG, "removeBond: $address already unbonded")
                return false
            }
            val ok = BluetoothDevice::class.java
                .getMethod("removeBond")
                .invoke(device) as? Boolean ?: false
            Log.i(TAG, "removeBond($address) -> $ok (was bondState=${device.bondState})")
            ok
        } catch (e: Exception) {
            Log.w(TAG, "removeBond($address) failed: ${e.message}")
            false
        }
    }
}
