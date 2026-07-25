package com.vortex.a3.service

import android.content.Intent
import android.util.Log
import java.security.SecureRandom

/**
 * Continuity Camera (phone → laptop): the laptop asks (AppState `cameraReq`)
 * to use this phone's camera as a webcam; we start [CameraStreamService] and
 * advertise where to dial + the media key in `cameraOffer`. Edge-tracked so the
 * repeated heartbeat level doesn't restart it. Extension fns on [VortexStack].
 */

private const val CAMERA_TAG = "VortexCamera"

/** Tracks the laptop's `cameraReq` level so we act only on the edges. */
@Volatile private var cameraReqActive = false

/** Consecutive `req == false` heartbeats while streaming. The laptop sends
 *  `cameraReq` over BOTH BLE and LAN; a stale pre-toggle `false` on the other
 *  transport must NOT stop→restart the camera (thrash). Debounce the stop. */
@Volatile private var cameraReqFalseMisses = 0
private const val CAMERA_FALSE_LIMIT = 3

/** The lens currently streaming, so a `cameraFacing` change flips the camera. */
@Volatile private var currentFacing = ""

/** Start/stop/flip the phone camera stream from the laptop's request + lens. */
internal fun VortexStack.handleCameraRequest(req: Boolean, facing: String) {
    if (req) {
        cameraReqFalseMisses = 0
    } else if (cameraReqActive && ++cameraReqFalseMisses < CAMERA_FALSE_LIMIT) {
        return // ignore a lone stale false — wait for a sustained release
    }
    // Lens flip while streaming: relaunch the service with the new facing
    // (same key/offer, so the laptop just reconnects). Default to front.
    val want = if (facing.isEmpty()) "front" else facing
    if (req && cameraReqActive && want != currentFacing) {
        currentFacing = want
        Log.i(CAMERA_TAG, "lens flip → $want")
        // Swap the lens in place — the encoder + TCP stream stay up. Republish
        // the offer with the new lens's rotation so the laptop re-rotates.
        VortexService.cameraOffer = VortexService.cameraOffer?.copy(
            rot = sensorRotation(want == "front"),
        )
        service.startService(
            Intent(service, CameraStreamService::class.java).apply {
                action = CameraStreamService.ACTION_FLIP
                putExtra(CameraStreamService.EXTRA_FACING, want)
            },
        )
        lanServer?.nudge()
        pushStateViaBle()
        return
    }
    if (req && !cameraReqActive) {
        cameraReqActive = true
        currentFacing = want
        // Fresh random media key; the laptop opens the sealed stream with it.
        val key = ByteArray(32).also { SecureRandom().nextBytes(it) }
        VortexService.cameraOffer = com.vortex.a3.core.appstate.CameraOffer(
            port = CameraStreamService.CAMERA_PORT,
            key = key.joinToString("") { "%02x".format(it) },
            rot = sensorRotation(want == "front"),
        )
        Log.i(CAMERA_TAG, "laptop requested camera ($want) → starting stream")
        val intent = Intent(service, CameraStreamService::class.java).apply {
            action = CameraStreamService.ACTION_START
            putExtra(CameraStreamService.EXTRA_KEY, key)
            putExtra(CameraStreamService.EXTRA_FACING, want)
        }
        androidx.core.content.ContextCompat.startForegroundService(service, intent)
        lanServer?.nudge() // ship the offer promptly
        pushStateViaBle()
    } else if (!req && cameraReqActive) {
        cameraReqActive = false
        cameraReqFalseMisses = 0
        currentFacing = ""
        VortexService.cameraOffer = null
        Log.i(CAMERA_TAG, "laptop released camera → stopping stream")
        service.startService(
            Intent(service, CameraStreamService::class.java).apply {
                action = CameraStreamService.ACTION_STOP
            },
        )
        lanServer?.nudge()
        pushStateViaBle()
    }
}

/** The chosen lens's sensor orientation (clockwise degrees to upright) — the
 *  laptop rotates frames by this. Queried without opening the camera. */
private fun VortexStack.sensorRotation(front: Boolean): Int = try {
    val mgr = service.getSystemService(android.content.Context.CAMERA_SERVICE)
        as android.hardware.camera2.CameraManager
    val want = if (front) {
        android.hardware.camera2.CameraMetadata.LENS_FACING_FRONT
    } else {
        android.hardware.camera2.CameraMetadata.LENS_FACING_BACK
    }
    val id = mgr.cameraIdList.firstOrNull {
        mgr.getCameraCharacteristics(it)
            .get(android.hardware.camera2.CameraCharacteristics.LENS_FACING) == want
    } ?: mgr.cameraIdList.firstOrNull()
    id?.let {
        mgr.getCameraCharacteristics(it)
            .get(android.hardware.camera2.CameraCharacteristics.SENSOR_ORIENTATION)
    } ?: 0
} catch (_: Throwable) {
    0
}
