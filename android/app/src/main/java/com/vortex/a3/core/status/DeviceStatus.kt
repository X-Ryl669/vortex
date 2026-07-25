package com.vortex.a3.core.status

import android.content.Context
import android.os.BatteryManager

/**
 * Device status payloads exchanged on the transport-keepalive channel.
 *
 * Mirrors `l3/daemon/src/core/status.rs`. The 2-byte tail appended to
 * the 8-byte ping/pong nonce carries:
 *
 *   ```text
 *   | 0 | 8 | random nonce                       |
 *   | 8 | 1 | battery %  (0..=100, 0xFF unknown) |
 *   | 9 | 1 | device_class                       |
 *   ```
 */
object DeviceClass {
    const val UNKNOWN: Byte = 0
    const val LAPTOP: Byte = 1
    const val PHONE: Byte = 2
    const val TABLET: Byte = 3
    const val EARBUDS: Byte = 4
}

/** Battery percentage 0..100, or null when the device can't report. */
data class BatteryPct(val value: Int?) {
    fun toByte(): Byte = (value ?: 0xFF).toByte()

    companion object {
        fun fromByte(b: Byte): BatteryPct {
            val v = b.toInt() and 0xFF
            return if (v == 0xFF || v > 100) BatteryPct(null) else BatteryPct(v)
        }
    }
}

data class LocalStatus(val battery: BatteryPct, val deviceClass: Byte) {
    /** Append the 2-byte tail to a payload that already holds the 8-byte nonce. */
    fun appendTo(buf: ByteArray, offset: Int) {
        buf[offset] = battery.toByte()
        buf[offset + 1] = deviceClass
    }
}

data class PeerStatus(val battery: BatteryPct, val deviceClass: Byte)

object DeviceStatusReader {
    /** Read this phone's battery percentage via BatteryManager. */
    fun readBattery(context: Context): BatteryPct {
        return try {
            val bm = context.applicationContext.getSystemService(BatteryManager::class.java)
            val v = bm?.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY) ?: -1
            if (v in 0..100) BatteryPct(v) else BatteryPct(null)
        } catch (_: Exception) {
            BatteryPct(null)
        }
    }

    /**
     * Whether the phone is currently plugged in / charging. Carried to the
     * peer so its UI can paint our battery blue. Treats CHARGING and FULL
     * (plugged in at 100%) as charging.
     */
    fun readCharging(context: Context): Boolean {
        return try {
            val bm = context.applicationContext.getSystemService(BatteryManager::class.java)
            val status = bm?.getIntProperty(BatteryManager.BATTERY_PROPERTY_STATUS)
            status == BatteryManager.BATTERY_STATUS_CHARGING ||
                status == BatteryManager.BATTERY_STATUS_FULL
        } catch (_: Exception) {
            false
        }
    }

    /** Build a local-side status for a typical Vortex phone. */
    fun localPhoneStatus(context: Context): LocalStatus =
        LocalStatus(readBattery(context), DeviceClass.PHONE)

    /** Decode the 2-byte tail of a peer's keepalive payload. */
    fun decodePeerStatus(payload: ByteArray): PeerStatus? {
        if (payload.size < 10) return null
        return PeerStatus(
            battery = BatteryPct.fromByte(payload[8]),
            deviceClass = payload[9],
        )
    }
}
