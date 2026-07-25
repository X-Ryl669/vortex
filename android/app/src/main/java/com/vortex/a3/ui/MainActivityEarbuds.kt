package com.vortex.a3.ui
import android.content.pm.PackageManager
import androidx.core.content.ContextCompat
import androidx.lifecycle.lifecycleScope
import kotlinx.coroutines.launch
import com.vortex.a3.service.VortexService
import com.vortex.a3.core.earbuds.BluetoothDeviceRow
import com.vortex.a3.core.earbuds.BluetoothScanner
import com.vortex.a3.core.earbuds.EarbudsStore
import com.vortex.a3.core.earbuds.SavedEarbuds

// MainActivity — Earbuds feature methods, split out of MainActivity.kt.
// Extension functions on the Activity so its (now `internal`) state stays in one
// instance; only the handler methods move here.

/**
 * Open the in-app earbuds picker. Acquires BT scan/connect
 * permission if needed, then kicks off a short BlueZ-equivalent
 * `BluetoothAdapter.startDiscovery()` sweep. The modal renders the
 * combined list (bonded ∪ freshly discovered) and the user picks
 * which device should "own" the earbuds card.
 */
internal fun MainActivity.openEarbudsPicker() {
    earbudsPickerState.value = PickerState(open = true, scanning = true, rows = emptyList())
    val needed = pickerRequiredPermissions().filter {
        ContextCompat.checkSelfPermission(this, it) != PackageManager.PERMISSION_GRANTED
    }
    if (needed.isEmpty()) {
        startPickerScan()
    } else {
        pickerPermissionLauncher.launch(needed.toTypedArray())
    }
}

/**
 * Force a re-scan from the modal's "Rescan" button. Same flow as
 * `openEarbudsPicker` minus the modal-open toggle.
 */
internal fun MainActivity.rescanEarbudsPicker() {
    if (earbudsPickerState.value.scanning) return
    startPickerScan()
}

/**
 * User picked a row in the modal — persist + close + nudge the
 * peer so the laptop's UI updates within ~200 ms instead of
 * waiting for the next 12 s heartbeat.
 */
internal fun MainActivity.pickEarbud(row: BluetoothDeviceRow) {
    EarbudsStore.save(applicationContext, SavedEarbuds(address = row.address, name = row.name))
    savedEarbudsExists.value = true
    earbudsPickerState.value = PickerState(open = false)
    // Refresh local earbuds state right away so the card updates without waiting for poll.
    lifecycleScope.launch {
        try {
            localEarbudsState.value = com.vortex.a3.core.earbuds.EarbudsDetector
                .readConnectedEarbuds(applicationContext)
        } catch (t: Throwable) {
            android.util.Log.w("VortexEarbuds", "post-pick refresh failed", t)
        }
        VortexService.requestLanNudge()
    }
}

/**
 * Long-press "Remove from Vortex" confirmed — drop the saved entry.
 * The bonded Bluetooth pairing on this device is left untouched.
 */
internal fun MainActivity.removeSavedEarbuds() {
    EarbudsStore.clear(applicationContext)
    savedEarbudsExists.value = false
    localEarbudsState.value = null
    VortexService.requestLanNudge()
}

internal fun MainActivity.requestSwitch() {
    val peer = peerStore.list().firstOrNull() ?: run {
        android.util.Log.w("VortexSwitch", "requestSwitch with no trusted peer")
        return
    }
    val saved = EarbudsStore.load(applicationContext) ?: run {
        android.util.Log.w("VortexSwitch", "requestSwitch with no saved earbuds")
        return
    }
    val mac = saved.address
    if (mac.isBlank()) {
        android.util.Log.w("VortexSwitch", "requestSwitch with empty saved mac")
        return
    }
    val budsOnLocal = localEarbudsState.value?.connected == true
    if (budsOnLocal) {
        android.util.Log.i("VortexSwitch", "swap: buds on local → asking peer to claim")
        VortexService.requestPeerToClaim()
    } else {
        val ok = com.vortex.a3.core.earbuds.EarbudsSwitchHolder
            .request(peer.peerStaticPub, mac)
        android.util.Log.i("VortexSwitch", "swap: claiming mac=$mac accepted=$ok")
    }
}

internal fun MainActivity.startPickerScan() {
    pickerScanJob?.cancel()
    earbudsPickerState.value = earbudsPickerState.value.copy(scanning = true)
    pickerScanJob = lifecycleScope.launch {
        val rows = try {
            BluetoothScanner.discover(applicationContext)
        } catch (t: Throwable) {
            android.util.Log.w("VortexEarbuds", "discover threw", t)
            emptyList()
        }
        // Don't overwrite a closed modal — user might have backed out.
        val s = earbudsPickerState.value
        if (s.open) {
            earbudsPickerState.value = s.copy(scanning = false, rows = rows)
        }
    }
}

internal fun MainActivity.closeEarbudsPicker() {
    pickerScanJob?.cancel()
    earbudsPickerState.value = PickerState(open = false)
}

internal fun MainActivity.refreshSavedEarbudsFlag() {
    savedEarbudsExists.value = EarbudsStore.load(applicationContext) != null
}
