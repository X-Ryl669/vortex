package com.vortex.a3.core.lan

import android.content.Context
import android.net.wifi.p2p.WifiP2pConfig
import android.net.wifi.p2p.WifiP2pManager
import android.os.Build
import android.os.Looper
import android.util.Log

/**
 * Wi-Fi Direct GROUP OWNER for instant-share fast transfer: the phone creates a
 * peer-to-peer Wi-Fi group (its own soft-AP, bypassing the router), the laptop
 * connects directly to it (just a normal Wi-Fi join to the known SSID/passphrase
 * — no wpa_supplicant P2P dance), and the existing [LanServer] (bound to all
 * interfaces) is reachable at the GO IP. 5 GHz for speed.
 *
 * VALIDATION STAGE: created via a debug broadcast; the laptop joins manually.
 * Fixed SSID/passphrase so the laptop side needs no negotiation yet.
 */
object WifiDirect {
    private const val TAG = "VortexWifiDirect"
    const val SSID = "DIRECT-vortex-xfer"
    const val PASS = "vortexdirect1234"
    /** Android always puts the P2P group owner at this address. */
    const val GO_IP = "192.168.49.1"

    private var manager: WifiP2pManager? = null
    private var channel: WifiP2pManager.Channel? = null

    /** True once the group owner is up (idempotent re-starts skip recreation). */
    @Volatile
    var isUp = false
        private set
    private var onReady: (() -> Unit)? = null

    /** Bring up the GO (idempotent); [onReady] fires once it's serving. */
    fun start(ctx: Context, onReady: (() -> Unit)? = null) {
        if (isUp) {
            onReady?.invoke()
            return
        }
        val m = ctx.applicationContext.getSystemService(Context.WIFI_P2P_SERVICE) as? WifiP2pManager
        if (m == null) {
            Log.w(TAG, "no WifiP2pManager")
            return
        }
        val c = m.initialize(ctx.applicationContext, Looper.getMainLooper(), null)
        manager = m
        channel = c
        this.onReady = onReady
        // Clear any stale group first, then (re)create regardless of the result.
        m.removeGroup(c, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = create(m, c, band5 = true)
            override fun onFailure(reason: Int) = create(m, c, band5 = true)
        })
    }

    private fun create(m: WifiP2pManager, c: WifiP2pManager.Channel, band5: Boolean) {
        val builder = WifiP2pConfig.Builder()
            .setNetworkName(SSID)
            .setPassphrase(PASS)
        if (band5) {
            builder.setGroupOperatingBand(WifiP2pConfig.GROUP_OWNER_BAND_5GHZ)
        }
        val config = builder.build()
        try {
            m.createGroup(c, config, object : WifiP2pManager.ActionListener {
                override fun onSuccess() {
                    isUp = true
                    Log.i(TAG, "WIFI_DIRECT GO UP: ssid=$SSID pass=$PASS ip=$GO_IP band=${if (band5) "5GHz" else "auto"}")
                    onReady?.invoke()
                    onReady = null
                }
                override fun onFailure(reason: Int) {
                    Log.w(TAG, "createGroup failed reason=$reason band5=$band5")
                    // 5 GHz unsupported on some chipsets → retry on auto band.
                    if (band5) create(m, c, band5 = false)
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "createGroup threw: ${e.message}")
        }
    }

    fun stop() {
        isUp = false
        onReady = null
        val c = channel ?: return
        manager?.removeGroup(c, object : WifiP2pManager.ActionListener {
            override fun onSuccess() { Log.i(TAG, "WIFI_DIRECT GO removed") }
            override fun onFailure(reason: Int) { Log.w(TAG, "removeGroup failed: $reason") }
        })
    }
}
