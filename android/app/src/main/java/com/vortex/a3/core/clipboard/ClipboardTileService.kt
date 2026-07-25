package com.vortex.a3.core.clipboard

import android.app.PendingIntent
import android.content.Intent
import android.os.Build
import android.service.quicksettings.TileService

/**
 * Quick Settings tile that sends THIS phone's clipboard to the laptop.
 * Tapping it launches [ClipboardQuickSendActivity] (foreground, so it may
 * read the clipboard — Android forbids background reads), which forwards the
 * text and finishes. The tile is the user gesture that makes phone→laptop
 * clipboard possible at all.
 */
class ClipboardTileService : TileService() {

    override fun onClick() {
        super.onClick()
        val launch = Intent(this, ClipboardQuickSendActivity::class.java).apply {
            addFlags(
                Intent.FLAG_ACTIVITY_NEW_TASK or
                    Intent.FLAG_ACTIVITY_NO_ANIMATION or
                    Intent.FLAG_ACTIVITY_EXCLUDE_FROM_RECENTS or
                    Intent.FLAG_ACTIVITY_CLEAR_TOP
            )
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            // Android 14+: collapsing the shade requires a PendingIntent.
            val pi = PendingIntent.getActivity(
                this,
                0,
                launch,
                PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
            )
            startActivityAndCollapse(pi)
        } else {
            @Suppress("DEPRECATION", "StartActivityAndCollapseDeprecated")
            startActivityAndCollapse(launch)
        }
    }
}
