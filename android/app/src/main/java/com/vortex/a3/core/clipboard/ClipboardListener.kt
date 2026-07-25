package com.vortex.a3.core.clipboard

import android.content.ClipboardManager
import android.content.Context
import android.os.Handler
import android.os.Looper
import android.util.Log
import com.vortex.a3.service.VortexService

/**
 * AUTOMATIC phone→laptop clipboard capture (universal-clipboard parity).
 *
 * Registers a [ClipboardManager.OnPrimaryClipChangedListener] in the foreground
 * service. On Android 10+ the OS only delivers this callback (and lets us read
 * `primaryClip`) to a BACKGROUND app when the `READ_CLIPBOARD` AppOp is granted
 * — see [ClipboardAccess]. When it isn't, the callback simply never fires in the
 * background and the user falls back to the Quick Settings tile; nothing breaks.
 *
 * TEXT only — images/URIs stay on the manual tile path (reading a foreign
 * `content://` image URI from a background service is unreliable). Forwards via
 * the SAME [VortexService.clipboardBus] the tile uses (which already handles
 * short + long text), so there's one send path.
 *
 * Loop-guarded: laptop→phone applies set [ClipboardSyncGuard]; a clip we just
 * applied is recognised here and NOT bounced back.
 */
class ClipboardListener(private val context: Context) {

    private val cm = context.getSystemService(Context.CLIPBOARD_SERVICE) as? ClipboardManager
    private val main = Handler(Looper.getMainLooper())
    private var listener: ClipboardManager.OnPrimaryClipChangedListener? = null

    fun start() = main.post {
        if (cm == null || listener != null) return@post
        val l = ClipboardManager.OnPrimaryClipChangedListener { onPrimaryClipChanged() }
        listener = l
        try {
            cm.addPrimaryClipChangedListener(l)
            Log.i(TAG, "auto clipboard capture armed (phone→laptop)")
        } catch (e: Exception) {
            listener = null
            Log.w(TAG, "failed to arm clipboard listener: ${e.message}")
        }
    }

    fun stop() = main.post {
        val l = listener ?: return@post
        try {
            cm?.removePrimaryClipChangedListener(l)
        } catch (_: Exception) {
        }
        listener = null
    }

    private fun onPrimaryClipChanged() {
        if (!ClipboardSyncSetting.isEnabled()) return
        // Background read may be blocked (no AppOp) — that throws/returns null,
        // which we treat as "use the tile" and silently ignore.
        val clip = try {
            cm?.primaryClip
        } catch (e: Exception) {
            return
        } ?: return
        if (ClipboardReader.isSensitive(clip)) {
            Log.i(TAG, "clipboard: sensitive item — not synced")
            return
        }
        if (clip.itemCount == 0) return
        val item = clip.getItemAt(0)
        // Auto-capture TEXT only. An image/file URI with no text → leave it to
        // the manual tile (the laptop→phone image path is the auto direction).
        if (item.uri != null && item.text.isNullOrEmpty()) return
        val text = item.text?.toString()?.trim().orEmpty()
        if (text.isEmpty()) return
        // Loop guard: don't bounce back what the laptop just sent us.
        if (ClipboardSyncGuard.wasJustApplied(ClipboardSyncGuard.sig(text))) return
        VortexService.clipboardBus.tryEmit(text)
        Log.i(TAG, "clipboard: auto-forwarded ${text.length} chars to laptop")
    }

    companion object {
        private const val TAG = "ClipboardListener"
    }
}
