package com.vortex.a3.core.clipboard

import android.content.ClipData
import android.content.ClipDescription
import android.os.Build

/**
 * Shared clipboard-read policy used by BOTH the auto-capture [ClipboardListener]
 * and the manual [ClipboardQuickSendActivity], so the rules live in one place.
 */
object ClipboardReader {
    /**
     * True if the clip is marked SENSITIVE ("concealed"-type parity).
     * Password managers / IMEs set [ClipDescription.EXTRA_IS_SENSITIVE] (API 33+)
     * on copied passwords, OTPs, card numbers, etc. We never sync those to the
     * laptop. Pre-API-33 the flag doesn't exist → treat as not-sensitive.
     */
    fun isSensitive(clip: ClipData?): Boolean {
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.TIRAMISU) return false
        return clip?.description?.extras
            ?.getBoolean(ClipDescription.EXTRA_IS_SENSITIVE, false) == true
    }
}
