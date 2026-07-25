package com.vortex.a3.core.clipboard

import android.app.AppOpsManager
import android.content.Context
import android.os.Process

/**
 * Detects whether THIS app may read the clipboard in the BACKGROUND — i.e.
 * whether phone→laptop clipboard sync is fully AUTOMATIC, or falls back to the
 * manual Quick Settings tile.
 *
 * Android 10+ forbids background clipboard reads for sideloaded apps unless the
 * `READ_CLIPBOARD` AppOp is explicitly set to ALLOW (via ADB or root) — the same
 * mechanism other BLE phone-link apps use. The user grants it once with:
 *
 *   adb shell appops set com.vortex.a3 READ_CLIPBOARD allow
 *
 * This is a UX hint only; the real ground truth is whether a background read
 * actually returns content. A force-stop / reinstall can reset the op on some
 * OEMs, so the listener degrades gracefully regardless.
 */
object ClipboardAccess {
    /** True if the AppOp is set to ALLOW (background reads work → auto sync). */
    fun isBackgroundReadGranted(context: Context): Boolean = try {
        val ops = context.getSystemService(Context.APP_OPS_SERVICE) as AppOpsManager
        val mode = ops.unsafeCheckOpNoThrow(
            "android:read_clipboard",
            Process.myUid(),
            context.packageName,
        )
        mode == AppOpsManager.MODE_ALLOWED
    } catch (_: Exception) {
        false
    }
}
