package com.vortex.a3.ui

import com.vortex.a3.core.ble.AdvPayload
import com.vortex.a3.core.earbuds.BluetoothDeviceRow

/**
 * Snapshot of the in-app BT picker modal. Top-level (not nested in
 * MainActivity) so the composables can take it in their parameter lists
 * without leaking the activity class through the API.
 */
data class PickerState(
    val open: Boolean = false,
    val scanning: Boolean = false,
    val rows: List<BluetoothDeviceRow> = emptyList(),
)

/**
 * Advertising state surfaced to the UI. Public so the extracted composables
 * can pattern-match against it without a dependency back into the Activity;
 * the values are only ever produced inside MainActivity.
 */
sealed class AdvertiseState {
    data object Idle : AdvertiseState()
    data object Starting : AdvertiseState()
    /** Pairable adv (open pairing window). */
    data class Active(val payload: AdvPayload) : AdvertiseState()
    /** Trusted-presence adv (rotating PRS-derived token). */
    data object TrustedPresence : AdvertiseState()
    data class Error(val reason: String) : AdvertiseState()
}
