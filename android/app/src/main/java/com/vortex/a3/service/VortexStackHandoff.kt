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
            // A Share is an EVENT; the AppState is a SNAPSHOT we re-send on
            // every heartbeat. Leaving the two mixed meant one Share sat in
            // the snapshot indefinitely, and the laptop — which acts on
            // `open_now` — opened the page again on every beat.
            //
            // The live browsing read is the part that genuinely IS state, and
            // it re-asserts itself every 25s, so dropping the share out of the
            // snapshot costs a pill nothing. Long enough first that a LAN-only
            // delivery still catches it; identity-checked so a newer handoff
            // that arrived meanwhile is never clobbered.
            if (ev.openNow && ev.url.isNotEmpty()) {
                scope.launch {
                    kotlinx.coroutines.delay(SHARE_SNAPSHOT_TTL_MS)
                    if (VortexService.currentHandoff === ev) {
                        VortexService.currentHandoff = null
                    }
                }
            }
            // BLE fast-path: the dedicated HANDOFF frame when the link is up.
            val server = gattServer ?: return@collect
            val json = ev.toJsonBytes()
            for (peer in peerStore.list()) {
                server.sendHandoffEncrypted(peer.peerStaticPub, json)
            }
        }
    }
}

/** How long an explicit Share stays in the outgoing AppState snapshot. Covers
 *  several heartbeats on either transport, so a share made while BLE is down
 *  still reaches the laptop over LAN. */
private const val SHARE_SNAPSHOT_TTL_MS = 30_000L
