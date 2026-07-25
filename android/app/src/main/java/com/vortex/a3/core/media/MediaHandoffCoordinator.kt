package com.vortex.a3.core.media

import android.content.ComponentName
import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.media.session.MediaController
import android.media.session.MediaSessionManager
import android.media.session.PlaybackState
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import android.view.KeyEvent

/**
 * Smart audio-follow: when media STARTS playing on this phone, pull the
 * earbuds over to the phone automatically. Mirrors the ecosystem's
 * MediaHandoffCoordinator, but ported onto Vortex's orchestrator seam —
 * a grab is just [requestGrab] (which runs the full AudioOp initiator
 * flow: Request → peer releases → local A2DP connect), so none of the
 * ecosystem's signal/watchdog plumbing is needed here.
 *
 * Decision (rising-edge only — see the plan):
 * ```
 * on local media just-started:
 *   skip if !enabled, a call is active, within a suppress window,
 *   pinned to the laptop, we already own the buds, or within cooldown
 *   else -> requestGrab()
 * ```
 * Anti-ping-pong: when the buds leave us (ownership true→false) we arm a
 * [LOSS_SUPPRESS_MS] window so a peer that just grabbed them isn't
 * immediately fought back. Continuous playback never re-grabs — only the
 * not-playing→playing edge does.
 *
 * Detection is reliable-with-fallback: [MediaSessionManager] (precise,
 * needs the user to enable notification access once) and, when that's
 * unavailable, [AudioManager.isMusicActive] (no special access). A
 * lightweight main-thread ticker reconciles state; the session/playback
 * listeners just nudge an immediate re-check for low latency.
 *
 * All callbacks/closures are invoked on the main thread.
 */
class MediaHandoffCoordinator(
    private val context: Context,
    /** True if the earbuds are currently connected to THIS phone. */
    private val weOwnBuds: () -> Boolean,
    /** True only when a hand-off actually makes sense: the peer (laptop)
     *  link is alive AND the peer currently holds the buds, so there's
     *  something to pull over. Gates the grab so that playing on the phone
     *  with no laptop around — or the buds sitting in their case — never
     *  pauses the phone's media for a hand-off that can't happen. */
    private val peerHoldsBuds: () -> Boolean,
    /** True if a phone call is ringing/active — calls outrank media. */
    private val isCallActive: () -> Boolean,
    /** Pull the buds to this phone (runs EarbudsSwitchHolder.request).
     *  Returns true if the switch actually STARTED; false if it was
     *  dropped (orchestrator busy / not ready) so we retry next tick
     *  instead of burning the cooldown. */
    private val requestGrab: () -> Boolean,
    /** Hand the buds back to the laptop (disconnect locally + claim peer)
     *  when our media has stopped for a couple of seconds. The laptop
     *  grabs them and resumes its own paused media. */
    private val requestReturnToPeer: () -> Unit,
    /** Publish local playing-state for the AppState heartbeat. */
    private val onMediaPlayingChanged: (Boolean) -> Unit,
) {
    @Volatile var smartSwitchEnabled: Boolean = true
    /** "phone" pins buds here; "laptop" suppresses grabbing them here;
     *  null = automatic. Set by the UI / a manual switch. */
    @Volatile var manualPreferredOwner: String? = null

    // --- Last-play-wins arbitration (mirrors the laptop media_watch) ---
    /** Peer's last-seen `media_playing`, set from the inbound AppState. */
    @Volatile var peerPlaying: Boolean = false
    /** Peer's play-start RE-ANCHORED to our elapsedRealtime clock
     *  (`now - peer_age`), set from the AppState. 0 when the peer isn't
     *  playing. When both sides play, the GREATER (more recent) keeps the
     *  buds. Clock-skew immune because it lives on OUR monotonic timeline. */
    @Volatile var peerPlayEpochMono: Long = 0L
    /** OUR play-start on the elapsedRealtime (monotonic) timeline, 0 = not
     *  playing. Frozen across hand-off pauses by [tick]. The heartbeat builder
     *  turns it into a relative AGE for the wire. */
    @Volatile var localPlayEpochMono: Long = 0L

    private val handler = Handler(Looper.getMainLooper())
    private var sessionManager: MediaSessionManager? = null
    private var audioManager: AudioManager? = null
    private val componentName = ComponentName(context, MediaNotificationListenerService::class.java)

    /** Whether the MediaSession path is usable (notification access granted). */
    private var sessionPathOk = false

    private var lastPlaying = false
    private var lastOwn = false
    private var lastAutoGrabMs = 0L
    private var suppressUntilMs = 0L
    private var running = false

    // Hand-off pause/resume. We pause the phone media in two cases:
    //   - forward grab: media started here, pause it so nothing blasts
    //     through the phone speaker while the buds move, then resume on
    //     arrival (`grabbing` is true → we advertise we want the buds);
    //   - lost the buds while playing: the laptop grabbed them, so pause +
    //     remember and resume from position when they return (`grabbing`
    //     false → we don't advertise wanting them).
    // `havePausedMedia` covers both; resume fires once the buds are ours
    // AND the output has fully migrated to them.
    private var havePausedMedia = false
    private var grabbing = false
    // Set when a forward grab overran RESUME_TIMEOUT_MS: the hold is kept
    // (packages still remembered) for GRAB_LATE_WINDOW_MS so a slow A2DP
    // connect still gets its resume. Cleared on resume / final drop.
    private var grabLate = false
    private var pausedAtMs = 0L
    // Return-to-laptop timer: armed when our media stops while we hold the
    // buds; fires after RETURN_DELAY_MS if nothing resumed.
    private var returnTimerAt = 0L
    // True ONLY while we hold buds we actively GRABBED from the laptop. We
    // return (disconnect locally + hand back) only these — if the buds were
    // simply already on the phone (no laptop, or the user put them here),
    // stopping media must NOT disconnect them. Set on grab, cleared on loss /
    // return / a non-grab gain.
    private var grabbedFromPeer = false
    // When we last GAINED the buds. The return timer can't arm for
    // RETURN_GRACE_MS afterwards, so a route-change auto-pause right after
    // a grab isn't mistaken for the user stopping (anti-oscillation).
    private var gainedAtMs = 0L
    // When the buds first became a ready output after a grab. Resume waits
    // OUTPUT_SETTLE_MS past this so the A2DP codec has actually started
    // flowing — otherwise the first 1-2s play into the void. Reset to 0
    // whenever the output isn't the buds.
    private var outputReadySinceMs = 0L
    private var lastAdvertised = false
    private val pausedPackages = mutableSetOf<String>()
    // Packages playing on the PREVIOUS tick. Used on a loss because Android
    // auto-pauses media the instant the A2DP route drops
    // (ACTION_AUDIO_BECOMING_NOISY) — before we can query — so a live query
    // at loss-time would catch nothing and we'd resume nothing.
    private val lastPlayingPackages = mutableSetOf<String>()
    // Audio-focus theft: used when the playing app exposes no controllable
    // MediaSession (so transportControls.pause() has nothing to target).
    private var audioFocusHeldForHandoff = false
    private var audioFocusRequest: AudioFocusRequest? = null
    // Pause enforcer: media apps auto-resume the instant we pause them, so
    // we re-pause on a tight loop until the buds arrive (mirrors the
    // ecosystem's mediaPauseEnforcer — the reason a single pause() sticks).
    private var enforcerActive = false
    private var enforcerStartedMs = 0L

    private val sessionsChanged =
        MediaSessionManager.OnActiveSessionsChangedListener { handler.post(::tick) }

    private val playbackCallback = object : AudioManager.AudioPlaybackCallback() {
        override fun onPlaybackConfigChanged(configs: MutableList<android.media.AudioPlaybackConfiguration>?) {
            handler.post(::tick)
        }
    }

    fun start() {
        if (running) return
        running = true
        audioManager = context.getSystemService(Context.AUDIO_SERVICE) as? AudioManager
        sessionManager = context.getSystemService(Context.MEDIA_SESSION_SERVICE) as? MediaSessionManager
        // Probe the MediaSession path. getActiveSessions throws
        // SecurityException unless our notification-listener component is
        // enabled — that's our signal to use the AudioManager fallback.
        sessionPathOk = try {
            sessionManager?.getActiveSessions(componentName)
            sessionManager?.addOnActiveSessionsChangedListener(sessionsChanged, componentName, handler)
            Log.i(TAG, "MediaSession detection active (notification access granted)")
            true
        } catch (e: SecurityException) {
            Log.i(TAG, "notification access not granted; using AudioManager fallback")
            false
        }
        audioManager?.registerAudioPlaybackCallback(playbackCallback, handler)
        // Seed ownership so the first tick doesn't see a spurious edge.
        lastOwn = weOwnBuds()
        handler.postDelayed(ticker, POLL_MS)
    }

    fun stop() {
        if (!running) return
        running = false
        handler.removeCallbacks(ticker)
        stopPauseEnforcer()
        abandonAudioFocusIfHeld()
        try {
            sessionManager?.removeOnActiveSessionsChangedListener(sessionsChanged)
        } catch (_: Exception) {}
        audioManager?.unregisterAudioPlaybackCallback(playbackCallback)
    }

    /** Arm the suppress window after a user-initiated manual switch so the
     *  auto logic doesn't immediately undo it. */
    fun noteManualSwitch() {
        suppressUntilMs = SystemClock.elapsedRealtime() + MANUAL_SUPPRESS_MS
    }

    private val ticker = object : Runnable {
        override fun run() {
            tick()
            if (running) handler.postDelayed(this, POLL_MS)
        }
    }

    private fun tick() {
        val now = SystemClock.elapsedRealtime()
        val playing = computePlaying()
        val own = weOwnBuds()

        // Snapshot the playing packages while we still can (Android pauses
        // them on the route drop before the loss handler runs).
        if (playing) {
            lastPlayingPackages.clear()
            lastPlayingPackages.addAll(currentPlayingPackages())
        }

        // Lost the buds (the laptop grabbed them). Arm anti-ping-pong and,
        // if we were playing, remember the players so we resume from
        // position when they come back.
        if (lastOwn && !own) {
            suppressUntilMs = now + LOSS_SUPPRESS_MS
            if (lastPlaying && !havePausedMedia) {
                pausePhoneMediaForLoss()
                Log.i(TAG, "buds left this phone while playing → remember ${pausedPackages.size} pkg")
                havePausedMedia = true
                pausedAtMs = now
            }
            grabbing = false
            grabbedFromPeer = false // the laptop has them now
        }
        if (!lastOwn && own) {
            gainedAtMs = now
            // If the buds arrived NOT via a grab we initiated, they're not
            // "borrowed" — stopping media here must never disconnect them.
            if (!grabbing) grabbedFromPeer = false
        }
        lastOwn = own

        // --- Last-play-wins epoch maintenance ---
        // Stamp a FRESH epoch only on a genuine user play-edge (playing rose
        // AND we're not mid-handoff). While havePausedMedia is set, a resume
        // is the SAME session recovering from our own switch-pause, so KEEP
        // the prior epoch (freeze) — that stops an auto-resume from out-bidding
        // the peer. A genuine stop clears it. MONOTONIC `now`
        // (elapsedRealtime), NOT wall-clock: we send a relative age, so the
        // epoch only ever lives on this device's own monotonic timeline.
        if (playing && !lastPlaying && !havePausedMedia) {
            localPlayEpochMono = now
        } else if (!playing && !havePausedMedia) {
            localPlayEpochMono = 0L
        }

        // Yield/loss enforcer: while we hold a hand-off pause record but do
        // NOT own the buds, any auto-resumed media is leaking to the phone
        // SPEAKER and about to mint a spurious epoch — re-pause it every tick
        // so it stays silent until the buds return or we resume.
        if (havePausedMedia && !own && playing) {
            reSilenceLeakingMedia()
        }

        // Resume what we paused — but only once the buds are ours AND the
        // output has fully migrated to them (forced, like the Linux side:
        // play through the buds, never the phone speaker). Or on timeout.
        // Track how long the buds have been a *settled* output. The A2DP
        // device shows up in getDevices() the instant it connects, but the
        // audio stream only actually flows ~1-2s later once the codec
        // negotiates. Resuming on the bare device-present signal plays the
        // first 1-2s into the void (or the phone speaker). So we require the
        // route to have been ready for OUTPUT_SETTLE_MS before resuming —
        // "play only after the output has fully moved to the earbuds".
        if (own && outputReady()) {
            if (outputReadySinceMs == 0L) outputReadySinceMs = now
        } else {
            outputReadySinceMs = 0L
        }
        val outputSettled =
            outputReadySinceMs != 0L && now - outputReadySinceMs >= OUTPUT_SETTLE_MS

        if (havePausedMedia) {
            // Convergence (#2): while we're holding for a loss/yield (NOT a
            // forward grab) and the laptop is STILL the more-recent player,
            // keep pushing the give-up timer forward so it never fires.
            // Otherwise the 90s LOSS_RESUME_TIMEOUT would clear the hold, our
            // media would auto-resume and mint a fresher epoch, and we'd
            // re-grab — a 90s-period bounce. Stay silent until the laptop
            // ACTUALLY stops (then the buds return → resume).
            if (!grabbing && !own) {
                val peerStillWinner = peerPlaying && peerPlayEpochMono != 0L &&
                    (localPlayEpochMono == 0L || peerPlayEpochMono > localPlayEpochMono)
                if (peerStillWinner) pausedAtMs = now
            }
            // Forward grab lands in ~1-2s (short timeout = give up). A
            // loss-remember (the laptop took the buds) waits much longer —
            // the buds return only when the laptop's media stops.
            val limit = when {
                grabbing -> RESUME_TIMEOUT_MS
                grabLate -> GRAB_LATE_WINDOW_MS
                else -> LOSS_RESUME_TIMEOUT_MS
            }
            if (own && outputReady() && outputSettled) {
                resumePhoneMedia()
                havePausedMedia = false
                grabbing = false
                grabLate = false
            } else if (now - pausedAtMs > limit) {
                if (grabbing) {
                    // The switch is likely still in flight (the orchestrator
                    // watchdog runs to ~14s; A2DP beats 6s only on a clean
                    // connect). Do NOT drop the remembered packages —
                    // downgrade to a bounded late window so the buds' LATE
                    // arrival still resumes. Dropping here was the "buds
                    // switched over but nothing played" bug.
                    grabbing = false
                    grabLate = true
                    pausedAtMs = now
                    Log.w(TAG, "buds slow to arrive; holding pause for a late resume")
                } else {
                    // STRICT sound-only-in-earbuds: the buds never arrived
                    // (out of range / grab failed / still on the laptop).
                    // Resuming here would blast the phone SPEAKER — stay
                    // silent instead and just forget the pause. The user's
                    // next play press re-runs the grab, and plays instantly
                    // if the buds showed up meanwhile.
                    Log.w(TAG, "hand-off resume timeout; staying paused (sound only in earbuds)")
                    havePausedMedia = false
                    grabLate = false
                    pausedPackages.clear()
                }
            }
        }

        // Media playing while the buds are elsewhere → pause IMMEDIATELY
        // (no speaker blast / lost intro) and grab. Catch-up: keeps
        // retrying until we own them.
        if (playing && !own) {
            maybeGrab(now)
        }

        // Return-to-laptop: our media stopped while we hold the buds. Arm
        // a short timer on the stop edge; if nothing resumes within
        // RETURN_DELAY_MS, hand the buds back so the laptop can resume its
        // own paused media.
        if (playing) {
            returnTimerAt = 0L
        } else if (returnTimerAt == 0L && own && grabbedFromPeer &&
            now - gainedAtMs >= RETURN_GRACE_MS
        ) {
            // Level-triggered (not edge): arm whenever the grabbed buds sit idle
            // past the grace, NOT only on the exact play→stop tick. The old
            // `lastPlaying` edge missed the case where media stopped DURING the
            // grace window — then later ticks have lastPlaying=false, so the
            // timer never armed and the buds were orphaned on the phone. The
            // `grabbedFromPeer` guard (we only return buds we grabbed) carries
            // the "a play happened" precondition `lastPlaying` used to, and the
            // `returnTimerAt == 0L` guard stops the countdown from resetting.
            returnTimerAt = now
        }
        if (returnTimerAt != 0L && own && !playing &&
            now - returnTimerAt >= RETURN_DELAY_MS
        ) {
            returnTimerAt = 0L
            // ONLY return buds we actually grabbed from the laptop. If they
            // were just on the phone (no laptop connected, or the user put them
            // here), stopping media must NOT disconnect them.
            if (grabbedFromPeer && smartSwitchEnabled && !isCallActive() &&
                manualPreferredOwner != "phone"
            ) {
                Log.i(TAG, "media stopped ${RETURN_DELAY_MS}ms ago → hand grabbed buds back to laptop")
                grabbedFromPeer = false
                requestReturnToPeer()
            }
        }
        // Nudge the laptop's now-playing pill on a play/pause edge — media
        // apps don't always repost their notification on a state flip, and
        // the pill's ⏸/▶ must track reality promptly.
        if (playing != lastPlaying) {
            MediaNotificationListenerService.rescanMediaPills()
        }
        lastPlaying = playing

        // Advertise "this is the media device" (handoff-aware) so the
        // laptop's release trigger stays valid. True while we're actively
        // grabbing, or we own the buds and are playing. NOT true just
        // because we have remembered-paused media (we lost the buds then).
        val advertised = grabbing || (playing && own)
        if (advertised != lastAdvertised) {
            lastAdvertised = advertised
            onMediaPlayingChanged(advertised)
        }
    }

    private fun maybeGrab(now: Long) {
        // Pure arbitration (unit-tested in MediaHandoffCoordinatorTest). The
        // query lambdas are side-effect-free reads, so evaluating them up front
        // for the decision is equivalent to the old short-circuit guard chain.
        // The suppress window honours loss-suppress (a player that auto-pauses/
        // plays on every route change would otherwise oscillate grab↔return).
        when (decideGrab(
            smartSwitchEnabled, isCallActive(), now, suppressUntilMs, manualPreferredOwner,
            weOwnBuds(), peerHoldsBuds(), peerPlaying, peerPlayEpochMono, localPlayEpochMono,
            lastAutoGrabMs,
        )) {
            GrabDecision.SKIP -> return
            // Last-play-wins: the laptop started its session MORE recently → it
            // won, so YIELD. Pause our media so it doesn't blast the phone
            // speaker (the tick re-silencer keeps it down) and do NOT grab; the
            // buds stay on the laptop. Converges dual-play instead of ping-pong.
            GrabDecision.YIELD -> {
                if (!havePausedMedia) {
                    pausePhoneMedia()
                    havePausedMedia = true
                    pausedAtMs = now
                    grabbing = false
                    Log.i(TAG, "peer played more recently → yield buds + pause local media")
                } else {
                    // A player started while an older hold is active (the
                    // re-silencer keeps it down). Merge it into the
                    // remembered set so the eventual resume targets it too.
                    pausedPackages.addAll(lastPlayingPackages)
                }
                return
            }
            GrabDecision.GRAB -> {} // fall through to the grab side-effects
        }
        // Pause the instant we decide to grab so the phone speaker never
        // plays the intro; resume once the buds are the active output.
        if (!havePausedMedia) {
            Log.i(TAG, "media on phone & buds elsewhere → pause + grab to phone")
            pausePhoneMedia()
            havePausedMedia = true
        } else {
            // Grabbing on top of an existing hold (earlier loss/yield): the
            // player the user just started isn't in the remembered set —
            // merge it so the arrival resume doesn't skip it.
            pausedPackages.addAll(lastPlayingPackages)
        }
        // (Re)start the resume-timeout clock from THIS grab, even when we
        // were ALREADY paused from an earlier loss/yield (port of the Linux
        // b7989b3 fix). A stale loss-era pausedAtMs makes RESUME_TIMEOUT_MS
        // expire on the very next tick — the hold is dropped before the
        // A2DP connect finishes, so the buds arrive and nothing resumes
        // ("pressed play, buds came over, silence" bug).
        pausedAtMs = now
        grabbing = true
        grabLate = false
        // decideGrab checked peerHoldsBuds() above, so this is a genuine pull
        // FROM the laptop → mark them borrowed so we return them when media stops.
        grabbedFromPeer = true
        // Only burn the cooldown if the switch actually STARTED. If the
        // orchestrator was busy (returned false) we leave the cooldown
        // clear so the next tick retries — instead of dropping the switch
        // for a full GRAB_COOLDOWN_MS, the bug behind "sometimes doesn't
        // switch".
        if (requestGrab()) {
            lastAutoGrabMs = now
        }
    }

    /** True once an actual Bluetooth output route exists — we only resume
     *  media after the output has fully migrated to the buds, so nothing
     *  leaks through the phone speaker. */
    private fun outputReady(): Boolean {
        val am = audioManager ?: return true
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return true
        return am.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any {
            it.type == AudioDeviceInfo.TYPE_BLUETOOTH_A2DP ||
                it.type == AudioDeviceInfo.TYPE_BLE_HEADSET ||
                it.type == AudioDeviceInfo.TYPE_BLUETOOTH_SCO
        }
    }

    /** Pause the phone media that triggered the grab, two layers deep:
     *  (1) transportControls.pause() on every playing session we can
     *  address, and (2) if the app exposes no controllable session, steal
     *  AUDIOFOCUS_GAIN so the default music app pauses. Then start the
     *  enforcer loop — apps auto-resume the instant we pause, so a single
     *  pause() never sticks; we re-pause on a tight loop until the buds
     *  arrive. This is the piece the ecosystem had that we were missing. */
    private fun pausePhoneMedia() {
        pausedPackages.clear()
        val sessions = activeSessions()
        for (c in sessions) {
            if (c.isPlaying()) {
                try { c.transportControls.pause() } catch (_: Exception) {}
                c.packageName?.let { pausedPackages.add(it) }
            }
        }
        // No controllable session but audio is still playing → steal focus.
        if (pausedPackages.isEmpty() && audioManager?.isMusicActive == true) {
            audioFocusHeldForHandoff = requestAudioFocusForHandoff()
        }
        startPauseEnforcer()
    }

    /** Packages whose session is currently Playing/Buffering. */
    private fun currentPlayingPackages(): Set<String> =
        activeSessions()
            .filter { it.isPlaying() }
            .mapNotNull { it.packageName?.trim()?.takeIf { p -> p.isNotEmpty() } }
            .toSet()

    /** Loss variant of [pausePhoneMedia]: Android has already auto-paused
     *  the media (route drop), so remember the previous tick's playing set
     *  rather than the now-empty live one. Best-effort pause any straggler
     *  + the enforcer + focus steal as usual. */
    private fun pausePhoneMediaForLoss() {
        pausedPackages.clear()
        pausedPackages.addAll(lastPlayingPackages)
        for (c in activeSessions()) {
            val pkg = c.packageName?.trim().orEmpty()
            if (pkg.isNotEmpty() && pkg in lastPlayingPackages && c.isPlaying()) {
                try { c.transportControls.pause() } catch (_: Exception) {}
            }
        }
        if (pausedPackages.isEmpty() && audioManager?.isMusicActive == true) {
            audioFocusHeldForHandoff = requestAudioFocusForHandoff()
        }
        startPauseEnforcer()
    }

    /** Per-tick re-pause used while we're yielding/awaiting a return and the
     *  buds aren't ours: media apps auto-resume the instant we let go, so a
     *  one-shot pause leaks to the phone speaker (and mints a spurious play
     *  epoch). Re-pause any playing session + steal focus if there's no
     *  controllable session. Does NOT touch [pausedPackages] (the resume set
     *  is already remembered) and runs straight from [tick]'s 150ms cadence,
     *  so it holds media silent tighter than the 2.8s pause-enforcer alone. */
    private fun reSilenceLeakingMedia() {
        var anySession = false
        for (c in activeSessions()) {
            if (c.isPlaying()) {
                anySession = true
                try { c.transportControls.pause() } catch (_: Exception) {}
            }
        }
        if (!anySession && audioManager?.isMusicActive == true && !audioFocusHeldForHandoff) {
            audioFocusHeldForHandoff = requestAudioFocusForHandoff()
        }
    }

    private fun startPauseEnforcer() {
        enforcerStartedMs = SystemClock.elapsedRealtime()
        enforcerActive = true
        handler.removeCallbacks(enforcer)
        handler.post(enforcer)
    }

    private fun stopPauseEnforcer() {
        enforcerActive = false
        handler.removeCallbacks(enforcer)
    }

    /** Re-pause the tracked sessions every [ENFORCER_TICK_MS] until the
     *  buds land here or [ENFORCER_TIMEOUT_MS] elapses. */
    private val enforcer = object : Runnable {
        override fun run() {
            if (!enforcerActive) return
            val elapsed = SystemClock.elapsedRealtime() - enforcerStartedMs
            if (weOwnBuds() || elapsed >= ENFORCER_TIMEOUT_MS) {
                enforcerActive = false
                return
            }
            if (pausedPackages.isNotEmpty()) {
                for (c in activeSessions()) {
                    val pkg = c.packageName?.trim().orEmpty()
                    if (pkg.isNotEmpty() && pkg in pausedPackages) {
                        try { c.transportControls.pause() } catch (_: Exception) {}
                    }
                }
            }
            handler.postDelayed(this, ENFORCER_TICK_MS)
        }
    }

    /** Resume the media we paused, once the buds are here. Stops the
     *  enforcer, gives back any stolen focus, then re-issues play across a
     *  safety-net window (apps need a beat after a profile switch, and a
     *  late route-migration wave can re-pause them seconds later — see
     *  [RESUME_REPLAY_CHECKPOINTS_MS]) with a media-key fallback.
     *
     *  Per-checkpoint the check is *action-based* and *per session*: it
     *  re-plays only the wanted sessions that are NOT currently playing,
     *  mirroring the Linux side's `player_is_playing` re-check, instead of
     *  bailing the whole round on the first sign of any audio (which could
     *  leave a second paused player silent and missed late re-pause waves
     *  past the old ~2.6s ceiling). */
    private fun resumePhoneMedia() {
        stopPauseEnforcer()
        val hadFocus = audioFocusHeldForHandoff
        if (hadFocus) abandonAudioFocusIfHeld()
        val wanted = pausedPackages.toSet()
        pausedPackages.clear()

        fun playPackages(targets: Set<String>): Int {
            var resumed = 0
            for (c in activeSessions()) {
                val pkg = c.packageName?.trim().orEmpty()
                if (pkg.isEmpty() || pkg !in targets) continue
                try { c.transportControls.play(); resumed++ } catch (_: Exception) {}
            }
            return resumed
        }

        fun mediaKeyFallback() {
            audioManager?.dispatchMediaKeyEvent(
                KeyEvent(KeyEvent.ACTION_DOWN, KeyEvent.KEYCODE_MEDIA_PLAY),
            )
            audioManager?.dispatchMediaKeyEvent(
                KeyEvent(KeyEvent.ACTION_UP, KeyEvent.KEYCODE_MEDIA_PLAY),
            )
        }

        for (delay in RESUME_REPLAY_CHECKPOINTS_MS) {
            handler.postDelayed({
                if (wanted.isEmpty()) {
                    // Focus-theft path: no addressable session to inspect,
                    // so fall back to the global output signal + media key.
                    if (!computePlaying()) mediaKeyFallback()
                } else {
                    // Re-play only the wanted sessions that fell back to
                    // paused; leave the ones already playing alone.
                    val notPlaying = wanted - currentPlayingPackages()
                    if (notPlaying.isNotEmpty()) {
                        val resumed = playPackages(notPlaying)
                        if (resumed == 0) mediaKeyFallback()
                        else Log.i(TAG, "resumed $resumed media session(s) on phone")
                    }
                }
            }, delay)
        }
    }

    private fun requestAudioFocusForHandoff(): Boolean {
        val am = audioManager ?: return false
        return if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val req = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                .setAudioAttributes(
                    AudioAttributes.Builder()
                        .setUsage(AudioAttributes.USAGE_MEDIA)
                        .setContentType(AudioAttributes.CONTENT_TYPE_MUSIC)
                        .build(),
                )
                .setOnAudioFocusChangeListener {}
                .build()
            audioFocusRequest = req
            am.requestAudioFocus(req) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        } else {
            @Suppress("DEPRECATION")
            am.requestAudioFocus(
                null,
                AudioManager.STREAM_MUSIC,
                AudioManager.AUDIOFOCUS_GAIN,
            ) == AudioManager.AUDIOFOCUS_REQUEST_GRANTED
        }
    }

    private fun abandonAudioFocusIfHeld() {
        if (!audioFocusHeldForHandoff) return
        audioFocusHeldForHandoff = false
        val am = audioManager ?: return
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            audioFocusRequest?.let { am.abandonAudioFocusRequest(it) }
            audioFocusRequest = null
        } else {
            @Suppress("DEPRECATION")
            am.abandonAudioFocus(null)
        }
    }

    /** Active media sessions, or empty if the MediaSession path is off.
     *  Our OWN package is excluded: [LaptopMediaNotification]'s proxy
     *  session (the laptop's now-playing) reports STATE_PLAYING without
     *  producing audio — pausing it here would remote-pause the LAPTOP. */
    private fun activeSessions(): List<MediaController> {
        if (!sessionPathOk) return emptyList()
        return try {
            (sessionManager?.getActiveSessions(componentName) ?: emptyList())
                .filter { it.packageName != context.packageName }
        } catch (e: SecurityException) {
            sessionPathOk = false
            emptyList()
        }
    }

    /** Is media ACTUALLY producing audio right now. We gate on
     *  [AudioManager.isMusicActive] — real output — instead of trusting
     *  MediaController.playbackState, because apps (Telegram, browsers)
     *  routinely leave a stale session stuck at STATE_PLAYING. A stale
     *  session would make us think media is always playing, so a real
     *  play never looks like a fresh start and we'd never grab. Sessions
     *  are still used (in pause/resume) to target the right app. */
    private fun computePlaying(): Boolean = audioManager?.isMusicActive == true

    private fun MediaController.isPlaying(): Boolean {
        val s = playbackState?.state ?: return false
        return s == PlaybackState.STATE_PLAYING || s == PlaybackState.STATE_BUFFERING
    }

    companion object {
        /** Decision from [decideGrab]: SKIP = a guard blocks (do nothing);
         *  YIELD = peer's media epoch is fresher (pause local, don't grab);
         *  GRAB = pull the buds to this device. */
        enum class GrabDecision { SKIP, YIELD, GRAB }

        /** Pure last-play-wins grab arbitration, extracted verbatim from
         *  [maybeGrab] so it's unit-testable without the Android audio stack.
         *  SAME guard order as the live path; the caller applies the side
         *  effects (pause / requestGrab). */
        fun decideGrab(
            smartSwitchEnabled: Boolean,
            callActive: Boolean,
            now: Long,
            suppressUntilMs: Long,
            manualPreferredOwner: String?,
            weOwnBuds: Boolean,
            peerHoldsBuds: Boolean,
            peerPlaying: Boolean,
            peerPlayEpochMono: Long,
            localPlayEpochMono: Long,
            lastAutoGrabMs: Long,
            cooldownMs: Long = GRAB_COOLDOWN_MS,
        ): GrabDecision {
            if (!smartSwitchEnabled) return GrabDecision.SKIP
            if (callActive) return GrabDecision.SKIP          // call #1 priority
            if (now < suppressUntilMs) return GrabDecision.SKIP
            if (manualPreferredOwner == "laptop") return GrabDecision.SKIP
            if (weOwnBuds) return GrabDecision.SKIP            // already ours
            if (!peerHoldsBuds) return GrabDecision.SKIP       // nothing to grab
            if (peerPlaying && peerPlayEpochMono != 0L && localPlayEpochMono != 0L &&
                peerPlayEpochMono > localPlayEpochMono
            ) {
                return GrabDecision.YIELD                      // laptop won the race
            }
            if (now - lastAutoGrabMs < cooldownMs) return GrabDecision.SKIP
            return GrabDecision.GRAB
        }

        private const val TAG = "MediaHandoff"

        /** Min gap between two auto-grabs. */
        private const val GRAB_COOLDOWN_MS = 4_000L
        /** After the buds leave us, don't fight to reclaim for this long. */
        private const val LOSS_SUPPRESS_MS = 4_000L
        /** After a manual switch, suppress auto logic this long. */
        private const val MANUAL_SUPPRESS_MS = 8_000L
        /** Reconcile interval. Close to the ecosystem's 120ms so detection
         *  + catch-up retry react fast; the session/playback listeners
         *  still nudge immediate re-checks on top. Cheap here (MediaSession
         *  / AudioManager queries, no subprocess). */
        private const val POLL_MS = 150L
        /** After our media stops while we hold the buds, wait this long
         *  before handing them back to the laptop (so a brief pause /
         *  track change doesn't bounce the buds). */
        private const val RETURN_DELAY_MS = 2_000L
        /** Grace after gaining the buds during which the return timer can't
         *  arm — covers a route-change auto-pause right after a grab so it
         *  isn't mistaken for the user stopping (anti-oscillation). */
        private const val RETURN_GRACE_MS = 3_000L
        /** If the buds don't arrive this long after a grab, resume the
         *  paused media anyway so it isn't stuck silent. */
        private const val RESUME_TIMEOUT_MS = 6_000L
        /** How long the buds must be the (present) output before we resume.
         *  The A2DP device appears the moment it connects, but the audio
         *  stream only flows ~1-2s later once the codec negotiates; resuming
         *  earlier loses the first seconds. Held just under RESUME_TIMEOUT_MS
         *  so the timeout fallback still wins if the route never settles. */
        private const val OUTPUT_SETTLE_MS = 1_200L
        /** Loss-remember resume timeout — the buds return when the laptop's
         *  media stops, which can be minutes; only a last-ditch un-stick. */
        private const val LOSS_RESUME_TIMEOUT_MS = 90_000L
        /** After a forward grab overruns [RESUME_TIMEOUT_MS], keep the
         *  remembered pause alive this much longer instead of dropping it:
         *  the orchestrator watchdog runs the switch out to ~14s, so under
         *  BT churn the buds routinely arrive after the 6s window — and
         *  dropping the record meant the late arrival resumed nothing. */
        private const val GRAB_LATE_WINDOW_MS = 15_000L
        /** Re-pause interval while the hand-off is in flight. */
        private const val ENFORCER_TICK_MS = 120L
        /** Max time the enforcer re-pauses before giving up. */
        private const val ENFORCER_TIMEOUT_MS = 2_800L
        /** Resume safety-net checkpoints (ms from the resume call). Mirrors
         *  the Linux 63eeaee schedule (media_watch.rs): WirePlumber / route
         *  migration fires auto-pause *waves* that can re-pause a player up
         *  to several seconds AFTER it resumed, so we keep re-checking out
         *  to ~7s — not just the first ~2.6s — re-issuing play only to the
         *  sessions that fell back. */
        private val RESUME_REPLAY_CHECKPOINTS_MS =
            longArrayOf(0L, 300L, 600L, 1_000L, 1_500L, 2_200L, 3_000L, 4_000L, 5_000L, 6_000L, 7_000L)
    }
}
