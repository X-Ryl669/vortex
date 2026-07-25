package com.vortex.a3.ui
import android.os.Build
import androidx.compose.material.icons.outlined.Settings
import android.bluetooth.BluetoothAdapter
import android.content.ComponentName
import android.content.Intent
import android.provider.Settings
import com.vortex.a3.service.VortexService
import com.vortex.a3.core.appstate.AppState
import com.vortex.a3.ui.components.EarbudsCard

// MainActivity — Settings feature methods, split out of MainActivity.kt.
// Extension functions on the Activity so its (now `internal`) state stays in one
// instance; only the handler methods move here.

/**
 * Triggered by EarbudsCard tap-to-bring or long-press send-to-peer.
 * V1 single-peer: we always target the first trusted peer. The mac
 * is the saved earbuds address (or the live one if no saved row).
 */
/**
 * Bidirectional swap. The icon doesn't care which side currently
 * holds the buds — the protocol picks the right direction:
 *
 *   • Buds on local → we send the peer an `audio_claim_request`
 *     flag via the next outgoing AppState heartbeat; the peer
 *     becomes initiator and pulls the buds.
 *   • Buds on peer (or anywhere off-local) → we are the initiator
 *     and run the normal request flow.
 */
/**
 * Remote lock/unlock for the laptop, from its home-screen card. One tap each
 * way — no extra biometric. The auth is implicit and consistent: to reach this
 * button the phone is necessarily unlocked (owner got past the keyguard), and
 * the laptop's proximity auto-unlock now ALSO gates on the phone being unlocked
 * (owner-present gate, AppState `unlocked`). So "phone unlocked & in hand" is the
 * authentication for both the manual button and the automatic path — a pocketed
 * /locked phone can open the laptop via neither.
 */
internal fun MainActivity.toggleLaptopLock(currentlyLocked: Boolean) {
    val op = if (currentlyLocked) "unlock" else "lock"
    VortexService.requestLaptopLock(applicationContext, op)
}

internal fun MainActivity.onRequestBatteryWhitelist() {
    // Two-step: try the in-app request dialog first (cleanest UX),
    // fall back to the system settings page if that intent isn't
    // handled by the OEM (some MIUI builds block ACTION_REQUEST).
    try {
        startActivity(
            Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
                data = android.net.Uri.parse("package:$packageName")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        )
        return
    } catch (_: Exception) {
        // fall through
    }
    try {
        startActivity(
            Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).apply {
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        )
    } catch (_: Exception) {
    }
}

internal fun MainActivity.onOpenAutostartSettings() {
    // Telegram / KDE-Connect-style: jump the user to the OEM's autostart /
    // "never-sleeping apps" whitelist screen. Stock Android / Pixel has no
    // equivalent (the standard receiver registration suffices); the aggressive
    // OEMs below block BOOT_COMPLETED / kill the background link until the user
    // whitelists the app here. We try every known OEM intent in order — only the
    // running device's own OEM package resolves, the rest throw and are skipped —
    // then fall back to generic app-info. A wrong/absent intent never crashes;
    // it just degrades to the next candidate (so it's safe on untested ROMs).
    val candidates = listOf(
        // Xiaomi / Redmi / Poco — MIUI / HyperOS
        ComponentName("com.miui.securitycenter", "com.miui.permcenter.autostart.AutoStartManagementActivity"),
        ComponentName("com.miui.securitycenter", "com.miui.appmanager.ApplicationsDetailsActivity"),
        // Huawei / Honor — EMUI
        ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.startupmgr.ui.StartupNormalAppListActivity"),
        ComponentName("com.huawei.systemmanager", "com.huawei.systemmanager.optimize.process.ProtectActivity"),
        // Samsung — One UI ("never-sleeping apps" under Device Care → Battery)
        ComponentName("com.samsung.android.lool", "com.samsung.android.sm.ui.battery.BatteryActivity"),
        ComponentName("com.samsung.android.lool", "com.samsung.android.sm.battery.ui.BatteryActivity"),
        ComponentName("com.samsung.android.sm", "com.samsung.android.sm.ui.battery.BatteryActivity"),
        // Oppo / Realme — ColorOS
        ComponentName("com.coloros.safecenter", "com.coloros.safecenter.permission.startup.StartupAppListActivity"),
        ComponentName("com.coloros.safecenter", "com.coloros.safecenter.startupapp.StartupAppListActivity"),
        ComponentName("com.oppo.safe", "com.oppo.safe.permission.startup.StartupAppListActivity"),
        // Vivo / iQOO — Funtouch / OriginOS
        ComponentName("com.vivo.permissionmanager", "com.vivo.permissionmanager.activity.BgStartUpManagerActivity"),
        ComponentName("com.iqoo.secure", "com.iqoo.secure.ui.phoneoptimize.AddWhiteListActivity"),
        // OnePlus — OxygenOS
        ComponentName("com.oneplus.security", "com.oneplus.security.chainlaunch.view.ChainLaunchAppListActivity"),
    )
    for (cn in candidates) {
        try {
            startActivity(
                Intent().apply {
                    component = cn
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
            )
            // Opening the Autostart screen IS the user acting on the
            // reminder. Since MIUI never reports back whether Autostart
            // got enabled, treat this tap as done and retire the standing
            // card automatically — no separate "Got it" needed.
            dismissAutostartHint()
            return
        } catch (e: Exception) {
            // try next
        }
    }
    // Fallback: app details settings.
    try {
        startActivity(
            Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                data = android.net.Uri.parse("package:$packageName")
                addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            }
        )
        dismissAutostartHint()
    } catch (e: Exception) {
        // no-op
    }
}

/** Permanently hide the MIUI-Autostart hint card. Called from its
 *  "Got it" button — the only way it clears, since Autostart can't be
 *  detected programmatically. */
internal fun MainActivity.dismissAutostartHint() {
    settingsPrefs.edit().putBoolean("autostart_hint_dismissed", true).apply()
    autostartHintDismissed.value = true
}

/** Ask the system to turn Bluetooth on (one-tap dialog). On Android 12+
 *  ACTION_REQUEST_ENABLE needs BLUETOOTH_CONNECT — if that's not yet granted
 *  it throws, so we fall back to the Bluetooth settings panel. Either way the
 *  launcher's result callback re-reads the adapter and clears the banner. */
internal fun MainActivity.onEnableBluetooth() {
    try {
        enableBluetoothLauncher.launch(Intent(BluetoothAdapter.ACTION_REQUEST_ENABLE))
        return
    } catch (_: Exception) {
        // SecurityException (missing BLUETOOTH_CONNECT) or no handler — fall through.
    }
    try {
        enableBluetoothLauncher.launch(
            Intent(Settings.ACTION_BLUETOOTH_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    } catch (_: Exception) { /* no-op */ }
}

/** Deep-link to the system "Accessibility" screen so the user can enable
 *  Vortex's screen-control injector (laptop → phone touch) with one tap. */
internal fun MainActivity.onOpenAccessibilitySettings() {
    try {
        startActivity(
            Intent(android.provider.Settings.ACTION_ACCESSIBILITY_SETTINGS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        )
    } catch (e: Exception) {
        try {
            startActivity(
                Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = android.net.Uri.parse("package:$packageName")
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
            )
        } catch (_: Exception) { /* no-op */ }
    }
}

/** Deep-link to the system "Notification access" screen so the user can
 *  enable Vortex's listener with one tap. */
internal fun MainActivity.onOpenNotificationAccess() {
    try {
        startActivity(
            Intent(android.provider.Settings.ACTION_NOTIFICATION_LISTENER_SETTINGS)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        )
    } catch (e: Exception) {
        // Fallback: app details (older OEMs).
        try {
            startActivity(
                Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS).apply {
                    data = android.net.Uri.parse("package:$packageName")
                    addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                }
            )
        } catch (_: Exception) { /* no-op */ }
    }
}
