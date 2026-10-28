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
        // The framework answers BUSY (2) to EVERY request while Wi-Fi is off,
        // which reads like a transient clash and hides the real cause. Say it.
        val wifi = ctx.applicationContext.getSystemService(Context.WIFI_SERVICE) as? android.net.wifi.WifiManager
        val wifiOn = wifi?.isWifiEnabled
        val p2pOk = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q) wifi?.isP2pSupported else null
        Log.i(TAG, "start: wifiEnabled=$wifiOn p2pSupported=$p2pOk")
        if (wifiOn == false) {
            Log.w(TAG, "Wi-Fi is OFF on this phone: Wi-Fi Direct cannot start (turn Wi-Fi on; it need not be connected)")
            return
        }
        val c = m.initialize(ctx.applicationContext, Looper.getMainLooper(), null)
        manager = m
        channel = c
        this.onReady = onReady
        // Clear any stale group first, then (re)create regardless of the result.
        m.removeGroup(c, object : WifiP2pManager.ActionListener {
            override fun onSuccess() = create(m, c, band5 = true)
            override fun onFailure(reason: Int) {
                Log.d(TAG, "pre-clean removeGroup: reason=$reason (no stale group is fine)")
                create(m, c, band5 = true)
            }
        })
    }

    /** BUSY (2) is often transient — a group still tearing down, a discovery in
     *  flight — so retry it a few times before concluding anything. */
    private const val BUSY = 2
    private const val MAX_BUSY_RETRIES = 4
    private const val BUSY_RETRY_DELAY_MS = 1500L
    private val handler = android.os.Handler(Looper.getMainLooper())

    private fun create(
        m: WifiP2pManager,
        c: WifiP2pManager.Channel,
        band5: Boolean,
        busyRetries: Int = 0,
    ) {
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
                    Log.w(TAG, "createGroup failed reason=$reason band5=$band5 busyRetries=$busyRetries")
                    if (reason == BUSY && busyRetries < MAX_BUSY_RETRIES) {
                        handler.postDelayed(
                            { create(m, c, band5, busyRetries + 1) },
                            BUSY_RETRY_DELAY_MS,
                        )
                    } else if (band5) {
                        // 5 GHz unsupported on some chipsets → retry on auto band.
                        create(m, c, band5 = false)
                    }
                }
            })
        } catch (e: Exception) {
            Log.w(TAG, "createGroup threw: ${e.message}")
        }
    }

    /** elapsedRealtime of the last frame over the group; 0 = none yet. */
    @Volatile
    private var lastActivityAtMs = 0L

    /** A frame just crossed the group link (see [LanServer]). Keeps it up. */
    fun noteActivity() {
        lastActivityAtMs = android.os.SystemClock.elapsedRealtime()
    }

    /** How long since the group link last carried a frame. */
    fun idleMs(): Long {
        val last = lastActivityAtMs
        if (last == 0L) return Long.MAX_VALUE
        return android.os.SystemClock.elapsedRealtime() - last
    }

    fun stop() {
        lastActivityAtMs = 0L
        isUp = false
        onReady = null
        val c = channel ?: return
        manager?.removeGroup(c, object : WifiP2pManager.ActionListener {
            override fun onSuccess() { Log.i(TAG, "WIFI_DIRECT GO removed") }
            override fun onFailure(reason: Int) { Log.w(TAG, "removeGroup failed: $reason") }
        })
    }
}
