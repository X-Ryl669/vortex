package com.vortex.a3.ui

import android.app.Activity
import android.content.Context
import android.os.Bundle
import android.util.Log
import android.view.GestureDetector
import android.view.Gravity
import android.view.MotionEvent
import android.view.ScaleGestureDetector
import android.view.SurfaceHolder
import android.view.SurfaceView
import android.view.WindowManager
import android.widget.FrameLayout
import com.vortex.a3.core.mirror.LaptopMirror
import com.vortex.a3.core.mirror.LaptopMirrorClient

/**
 * Fullscreen viewer for the LAPTOP's screen (laptop→phone mirror). Launched by
 * [LaptopMirror] once the laptop accepts our "view laptop screen" request. Hosts
 * an aspect-ratio-correct [SurfaceView] (16:9 — the laptop sends 720p, so we
 * letterbox instead of stretching) and drives a [LaptopMirrorClient] decode loop
 * on a worker thread. Pinch to zoom, drag to pan, double-tap to reset.
 */
class LaptopMirrorActivity : Activity() {
    private var client: LaptopMirrorClient? = null
    private var worker: Thread? = null

    // Zoom/pan transform state, applied to the SurfaceView.
    private var scale = 1f
    private lateinit var surface: AspectRatioSurfaceView

    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        window.addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON)

        val port = intent.getIntExtra(EXTRA_PORT, 0)
        val key = intent.getByteArrayExtra(EXTRA_KEY)
        if (port == 0 || key == null || key.size != 32) {
            Log.w(TAG, "missing/invalid launch params — finishing")
            finish()
            return
        }

        // Black backdrop + a centered, aspect-correct surface (letterboxed).
        val root = FrameLayout(this).apply { setBackgroundColor(0xFF000000.toInt()) }
        surface = AspectRatioSurfaceView(this).apply {
            layoutParams = FrameLayout.LayoutParams(
                FrameLayout.LayoutParams.WRAP_CONTENT,
                FrameLayout.LayoutParams.WRAP_CONTENT,
                Gravity.CENTER,
            )
        }
        root.addView(surface)
        setContentView(root)

        // Let the stack close us when the laptop stops casting.
        LaptopMirror.viewerCloser = { runOnUiThread { finish() } }

        attachZoomPan(root)

        surface.holder.addCallback(object : SurfaceHolder.Callback {
            override fun surfaceCreated(holder: SurfaceHolder) {
                val c = LaptopMirrorClient(port, key, holder.surface)
                client = c
                worker = Thread({ c.start() }, "laptop-mirror-view").also { it.start() }
            }

            override fun surfaceChanged(holder: SurfaceHolder, format: Int, w: Int, h: Int) {}

            override fun surfaceDestroyed(holder: SurfaceHolder) {
                stopClient()
            }
        })
    }

    /** Pinch-to-zoom (1x–5x), drag-to-pan while zoomed, double-tap to reset. */
    private fun attachZoomPan(root: FrameLayout) {
        val scaleDetector = ScaleGestureDetector(
            this,
            object : ScaleGestureDetector.SimpleOnScaleGestureListener() {
                override fun onScale(d: ScaleGestureDetector): Boolean {
                    scale = (scale * d.scaleFactor).coerceIn(1f, 5f)
                    surface.scaleX = scale
                    surface.scaleY = scale
                    clampPan()
                    return true
                }
            },
        )
        val panDetector = GestureDetector(
            this,
            object : GestureDetector.SimpleOnGestureListener() {
                override fun onScroll(
                    e1: MotionEvent?,
                    e2: MotionEvent,
                    dx: Float,
                    dy: Float,
                ): Boolean {
                    if (scale > 1f) {
                        surface.translationX -= dx
                        surface.translationY -= dy
                        clampPan()
                    }
                    return true
                }

                override fun onDoubleTap(e: MotionEvent): Boolean {
                    scale = 1f
                    surface.scaleX = 1f
                    surface.scaleY = 1f
                    surface.translationX = 0f
                    surface.translationY = 0f
                    return true
                }
            },
        )
        root.setOnTouchListener { _, ev ->
            scaleDetector.onTouchEvent(ev)
            panDetector.onTouchEvent(ev)
            true
        }
    }

    /** Keep the zoomed surface from being dragged past its own edges. */
    private fun clampPan() {
        val maxX = (surface.width * (scale - 1f)) / 2f
        val maxY = (surface.height * (scale - 1f)) / 2f
        surface.translationX = surface.translationX.coerceIn(-maxX, maxX)
        surface.translationY = surface.translationY.coerceIn(-maxY, maxY)
    }

    override fun onDestroy() {
        super.onDestroy()
        LaptopMirror.viewerCloser = null
        stopClient()
        // Viewer gone → tell the stack to stop requesting the cast (the laptop
        // sees the request drop and releases its screen capture + portal).
        LaptopMirror.onViewerClosed(applicationContext)
    }

    private fun stopClient() {
        client?.stop()
        client = null
        worker?.let { try { it.join(500) } catch (_: Throwable) {} }
        worker = null
    }

    companion object {
        private const val TAG = "LaptopMirror"
        const val EXTRA_PORT = "port"
        const val EXTRA_KEY = "key"
    }
}

/**
 * A [SurfaceView] that measures itself to a fixed aspect ratio (default 16:9,
 * the laptop's 720p stream) and fits inside the available space — so the video
 * is letterboxed, never stretched.
 */
private class AspectRatioSurfaceView(context: Context) : SurfaceView(context) {
    private val aspectW = 16
    private val aspectH = 9

    override fun onMeasure(widthMeasureSpec: Int, heightMeasureSpec: Int) {
        val availW = MeasureSpec.getSize(widthMeasureSpec)
        val availH = MeasureSpec.getSize(heightMeasureSpec)
        if (availW == 0 || availH == 0) {
            super.onMeasure(widthMeasureSpec, heightMeasureSpec)
            return
        }
        val target = aspectW.toFloat() / aspectH
        val avail = availW.toFloat() / availH
        val (w, h) = if (avail > target) {
            // Parent wider than the video → letterbox left/right, limit by height.
            ((availH * target).toInt()) to availH
        } else {
            // Parent taller → limit by width.
            availW to ((availW / target).toInt())
        }
        setMeasuredDimension(w, h)
    }
}
