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

    // Matches `input_proto` in the laptop's mirror.rs — one protocol for both
    // directions rather than a second one to keep in step.
    private val INPUT_DOWN = 0
    private val INPUT_MOVE = 1
    private val INPUT_UP = 2
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
                val c = LaptopMirrorClient(port, key, holder.surface) { w, h ->
                    runOnUiThread { surface.setAspect(w, h) }
                }
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
            // ONE finger drives the laptop; TWO keep the pan and zoom above.
            //
            // The split matters: on a phone-sized screen showing a laptop
            // desktop, pan and zoom are what make the thing usable at all, so
            // they cannot be given up to make it interactive. A second finger
            // arriving mid-drag also has to LIFT the laptop's button, or the
            // desktop is left mid-drag while the user is pinching.
            if (ev.pointerCount == 1) {
                sendTouch(ev)
            } else if (laptopButtonDown) {
                sendInput(INPUT_UP, ev.getX(0), ev.getY(0))
                laptopButtonDown = false
            }
            true
        }
    }

    /** Is the laptop currently holding a button down because of us? */
    private var laptopButtonDown = false

    private fun sendTouch(ev: MotionEvent) {
        when (ev.actionMasked) {
            MotionEvent.ACTION_DOWN -> {
                sendInput(INPUT_DOWN, ev.x, ev.y)
                laptopButtonDown = true
            }
            MotionEvent.ACTION_MOVE -> if (laptopButtonDown) sendInput(INPUT_MOVE, ev.x, ev.y)
            MotionEvent.ACTION_UP, MotionEvent.ACTION_CANCEL -> {
                if (laptopButtonDown) {
                    sendInput(INPUT_UP, ev.x, ev.y)
                    laptopButtonDown = false
                }
            }
        }
    }

    /**
     * Turn a point on the VIDEO into a normalised position and send it.
     *
     * The surface can be panned and zoomed, so a raw touch coordinate is not a
     * point on the laptop's screen: the transform has to be undone first, or
     * every tap lands somewhere else once the user has zoomed in. The result is
     * normalised to 0..65535 so the phone never needs the monitor's real size.
     */
    private fun sendInput(type: Int, rawX: Float, rawY: Float) {
        val c = client ?: return
        val w = surface.width.toFloat()
        val h = surface.height.toFloat()
        if (w <= 0f || h <= 0f) return
        // Undo pan, then zoom, about the surface's centre — the same origin the
        // scale is applied around.
        val cx = w / 2f
        val cy = h / 2f
        val x = (rawX - surface.translationX - cx) / scale + cx
        val y = (rawY - surface.translationY - cy) / scale + cy
        // Outside the picture after the transform: a touch on the letterbox is
        // not a touch on the laptop.
        if (x < 0f || y < 0f || x > w || y > h) return
        val nx = ((x / w) * 65535f).toInt().coerceIn(0, 65535)
        val ny = ((y / h) * 65535f).toInt().coerceIn(0, 65535)
        c.sendInput(type, nx, ny)
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
    // 16:9 only until the decoder tells us otherwise. In extend mode the laptop
    // sends a portrait monitor, and assuming landscape stretched it badly.
    private var aspectW = 16
    private var aspectH = 9

    fun setAspect(w: Int, h: Int) {
        if (w <= 0 || h <= 0 || (w == aspectW && h == aspectH)) return
        aspectW = w
        aspectH = h
        requestLayout()
    }

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
