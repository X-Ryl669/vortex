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
            // An explicit Share is a one-shot COMMAND, not state, so its carry
            // has to EXPIRE. It still rides the AppState snapshot so it survives
            // a dead BLE link, but that snapshot is republished on every
            // heartbeat: left in place it told the laptop to open the page
            // forever — one browser tab every ~12s for as long as this app
            // stayed up. Nothing the user did on the phone stopped it, because
            // only the accessibility read ever writes an empty url, and copying
            // other text doesn't touch this bus at all.
            //
            // The window only has to outlast one heartbeat. The laptop also
            // dedups on `id`, so a late or doubled delivery is harmless — this
            // is about not generating the traffic in the first place.
            if (ev.openNow) {
                scope.launch {
                    kotlinx.coroutines.delay(OPEN_NOW_CARRY_MS)
                    // Retract only OUR event: a newer page (or a pill clear) may
                    // have landed in the meantime and must not be clobbered.
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

/** How long an `openNow` Share stays in the AppState snapshot. Long enough for
 *  a BLE-down laptop to pick it up off a LAN heartbeat, short enough that a
 *  missed retraction cannot become a browser loop. */
private const val OPEN_NOW_CARRY_MS = 45_000L
