package com.vortex.a3.service

import kotlinx.coroutines.launch

/**
 * Browsing HANDOFF (seamless-continuity): forward a page the phone wants to
 * continue on the laptop. The Share sheet emits to [VortexService.handoffBus];
 * we ship each over BLE (HANDOFF frame) to the paired laptop, which opens it.
 *
 * User-initiated (an explicit Share), so it's NOT gated by the notification-
 * mirror toggle. Extension function on [VortexStack].
 */
internal fun VortexStack.forwardHandoff() {
    scope.launch {
        VortexService.handoffBus.collect { ev ->
            // Stash for the AppState LAN backstop FIRST — even when BLE is down
            // (that's exactly when the LAN path must carry it). Empty url clears.
            VortexService.currentHandoff = if (ev.url.isEmpty()) null else ev
            // BLE fast-path: the dedicated HANDOFF frame when the link is up.
            val server = gattServer ?: return@collect
            val json = ev.toJsonBytes()
            for (peer in peerStore.list()) {
                server.sendHandoffEncrypted(peer.peerStaticPub, json)
            }
        }
    }
}
