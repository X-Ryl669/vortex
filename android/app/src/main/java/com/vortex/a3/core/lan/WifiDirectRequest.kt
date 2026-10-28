package com.vortex.a3.core.lan

/**
 * Rising-edge filter for the laptop's "raise your Wi-Fi Direct group" request
 * (`AppState.wifiDirectReqSeq`). Same contract as `RingController`: the first
 * value seen only sets the baseline, so a request from before we connected
 * never brings up a group on connect, and the laptop's millis-valued stamp only
 * ever grows, so a laptop restart cannot regress into a false edge.
 */
object WifiDirectRequest {
    private var lastSeq = -1L

    @Synchronized
    fun isNew(seq: Long): Boolean {
        if (lastSeq < 0L) {
            lastSeq = seq
            return false
        }
        if (seq <= lastSeq) return false
        lastSeq = seq
        return true
    }
}
