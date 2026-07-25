package com.vortex.a3.service

import kotlinx.coroutines.launch

/** Last handled notif-invoke seq, so the SAME laptop→phone action/reply invoke
 *  arriving over BOTH the BLE NOTIFICATION frame and the LAN AppState backstop
 *  fires exactly once. */
@Volatile private var lastHandledNotifInvokeSeq = 0L

/**
 * Fire a laptop→phone notification action/reply INVOKE (Reply / Mark-as-read /
 * …), deduped by [com.vortex.a3.core.notif.NotificationMirror.seq] across the
 * two transports (BLE fast-path + LAN backstop). Called from both the BLE
 * NOTIFICATION handler and the AppState path.
 */
internal fun VortexStack.handleNotifInvoke(m: com.vortex.a3.core.notif.NotificationMirror) {
    if (m.invokeIndex < 0 || m.key.isEmpty()) return
    if (m.seq > 0L) {
        synchronized(this) {
            if (m.seq <= lastHandledNotifInvokeSeq) return
            lastHandledNotifInvokeSeq = m.seq
        }
    }
    com.vortex.a3.core.media.MediaNotificationListenerService
        .invokeAction(m.key, m.invokeIndex, m.reply)
}

/**
 * Phone→laptop notification mirroring — split out of [VortexStack]. Forwards
 * captured phone notifications to the paired laptop over BLE (NOTIFICATION
 * frame); BLE-only because desktop alerts want the low-latency persistent link,
 * not the LAN heartbeat. Buffered in the outbox while the link is down so none
 * are lost across drops. Gated by the local toggle; content is never logged.
 */
internal fun VortexStack.forwardNotifications() {
    scope.launch {
        VortexService.notificationBus.collect { mirror ->
            if (!com.vortex.a3.core.notif.NotificationMirrorSetting.isEnabled()) return@collect
            val json = mirror.toJsonBytes()
            for (peer in peerStore.list()) {
                val peerHex = peer.peerStaticPub.notifHex()
                val server = gattServer
                // BLE-only path: if the link is down (no server / no
                // subscriber / no cipher) the send fails — buffer it so it
                // flushes the moment the peer re-subscribes, instead of
                // vanishing during the outage.
                val sent = server?.sendNotificationEncrypted(peer.peerStaticPub, json) ?: false
                if (!sent) notificationOutbox.enqueue(peerHex, mirror)
            }
            // Push the app's real icon once per session (chunked) so the
            // laptop can show its logo. Fire-and-forget; re-sent on the
            // next post / reconnect until the laptop has it cached.
            val pkg = mirror.appId
            if (pkg.isNotEmpty() && !mirror.dismiss && sentIconPkgs.add(pkg)) {
                scope.launch { sendAppIcon(pkg) }
            }
        }
    }
}
