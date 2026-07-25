package com.vortex.a3.service

import android.util.Log
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.withLock

/**
 * Phone-companion mirrors (Contacts / Recents / SMS) — split out of
 * [VortexStack]. Each provider reads its content + observes changes, emits to a
 * VortexService bus, and this file forwards each snapshot to the laptop over BLE
 * as chunked frames, gated by the per-dataset local toggle and skipped when the
 * LAN bulk-sync already delivered the same hash. Extension functions on
 * [VortexStack] so the (now `internal`) shared state stays in one instance.
 */

/** Start the contacts provider (reads ContactsContract + observes changes)
 *  and forward each snapshot to the laptop over BLE (chunked CONTACTS
 *  frames), gated by the local [com.vortex.a3.core.contacts.ContactsMirrorSetting]. */
internal fun VortexStack.startContacts() {
    com.vortex.a3.core.contacts.ContactsMirrorSetting.init(ctx)
    // Provider callback → bus (replay=1 so a late re-subscribe re-sends).
    contactsProvider = com.vortex.a3.core.contacts.ContactsProvider(ctx) { contacts ->
        VortexService.contactsBus.tryEmit(contacts)
    }.also { it.start() }
    scope.launch {
        VortexService.contactsBus.collect { contacts ->
            if (!com.vortex.a3.core.contacts.ContactsMirrorSetting.isEnabled()) return@collect
            val json = com.vortex.a3.core.contacts.contactsToJsonBytes(contacts)
            // Keep the latest snapshot + hash for the LAN bulk-sync
            // responder (LanServer asks "does the laptop's cached hash
            // still match?") and gate the BLE burst: when the laptop
            // already has THIS exact JSON — confirmed by a LAN bulk
            // "sent"/"match" — re-shipping ~N chunks over BLE buys
            // nothing. In a BLE-only environment lanDelivered goes stale
            // the moment contacts change, so real updates still flow.
            val hash = sha256Hex(json)
            latestContactsJson = json
            latestContactsHash = hash
            if (hash == lanDeliveredContactsHash) {
                Log.i(VortexStack.TAG, "contacts unchanged since LAN bulk delivery; skipping BLE burst")
                return@collect
            }
            sendContacts(json)
        }
    }
}

/** Chunk + send a contacts-list JSON blob to every peer over BLE.
 *  Payload per frame: [total u16 BE][idx u16 BE][json-chunk]. */
internal suspend fun VortexStack.sendContacts(json: ByteArray) = companionSendMutex.withLock {
    val total = ((json.size + CONTACTS_CHUNK - 1) / CONTACTS_CHUNK).coerceAtLeast(1)
    if (total > 0xFFFF) return@withLock
    for (peer in peerStore.list()) {
        val server = gattServer ?: return@withLock
        for (idx in 0 until total) {
            val start = idx * CONTACTS_CHUNK
            val end = minOf(start + CONTACTS_CHUNK, json.size)
            val payload = java.io.ByteArrayOutputStream().apply {
                write((total ushr 8) and 0xFF); write(total and 0xFF)
                write((idx ushr 8) and 0xFF); write(idx and 0xFF)
                write(json, start, end - start)
            }.toByteArray()
            if (!server.sendContactsChunkEncrypted(peer.peerStaticPub, payload)) return@withLock
            kotlinx.coroutines.delay(20) // pace so the BLE stack doesn't drop notifies
        }
    }
    Log.i(VortexStack.TAG, "contacts sent ($total chunks, ${json.size} bytes)")
    kotlinx.coroutines.delay(800) // settle the BLE link before the next bulk burst
}

/** Start the call-log provider (reads CallLog.Calls + observes changes) and
 *  forward each snapshot to the laptop over BLE (chunked CALL_LOG frames),
 *  gated by the local [com.vortex.a3.core.calllog.CallLogMirrorSetting]. */
internal fun VortexStack.startCallLog() {
    com.vortex.a3.core.calllog.CallLogMirrorSetting.init(ctx)
    callLogProvider = com.vortex.a3.core.calllog.CallLogProvider(ctx) { entries ->
        VortexService.callLogBus.tryEmit(entries)
    }.also { it.start() }
    scope.launch {
        VortexService.callLogBus.collect { entries ->
            if (!com.vortex.a3.core.calllog.CallLogMirrorSetting.isEnabled()) return@collect
            val json = com.vortex.a3.core.calllog.callLogToJsonBytes(entries)
            // Same LAN bulk-sync bookkeeping + BLE-burst gate as contacts.
            val hash = sha256Hex(json)
            latestCallLogJson = json
            latestCallLogHash = hash
            if (hash == lanDeliveredCallLogHash) {
                Log.i(VortexStack.TAG, "call log unchanged since LAN bulk delivery; skipping BLE burst")
                return@collect
            }
            sendCallLog(json)
        }
    }
}

/** Chunk + send a call-log JSON blob to every peer over BLE.
 *  Payload per frame: [total u16 BE][idx u16 BE][json-chunk]. */
internal suspend fun VortexStack.sendCallLog(json: ByteArray) = companionSendMutex.withLock {
    val total = ((json.size + CONTACTS_CHUNK - 1) / CONTACTS_CHUNK).coerceAtLeast(1)
    if (total > 0xFFFF) return@withLock
    for (peer in peerStore.list()) {
        val server = gattServer ?: return@withLock
        for (idx in 0 until total) {
            val start = idx * CONTACTS_CHUNK
            val end = minOf(start + CONTACTS_CHUNK, json.size)
            val payload = java.io.ByteArrayOutputStream().apply {
                write((total ushr 8) and 0xFF); write(total and 0xFF)
                write((idx ushr 8) and 0xFF); write(idx and 0xFF)
                write(json, start, end - start)
            }.toByteArray()
            if (!server.sendCallLogChunkEncrypted(peer.peerStaticPub, payload)) return@withLock
            kotlinx.coroutines.delay(20) // pace so the BLE stack doesn't drop notifies
        }
    }
    Log.i(VortexStack.TAG, "call log sent ($total chunks, ${json.size} bytes)")
    kotlinx.coroutines.delay(800) // settle the BLE link before the next bulk burst
}

/** Start the SMS provider (reads Telephony.Sms + observes changes) and
 *  forward each snapshot to the laptop over BLE (chunked SMS frames), gated
 *  by the local [com.vortex.a3.core.sms.SmsMirrorSetting]. */
internal fun VortexStack.startSms() {
    com.vortex.a3.core.sms.SmsMirrorSetting.init(ctx)
    smsProvider = com.vortex.a3.core.sms.SmsProvider(ctx) { messages ->
        VortexService.smsBus.tryEmit(messages)
    }.also { it.start() }
    scope.launch {
        VortexService.smsBus.collect { messages ->
            if (!com.vortex.a3.core.sms.SmsMirrorSetting.isEnabled()) return@collect
            val json = com.vortex.a3.core.sms.smsToJsonBytes(messages)
            // Same LAN bulk-sync bookkeeping + BLE-burst gate as contacts.
            val hash = sha256Hex(json)
            latestSmsJson = json
            latestSmsHash = hash
            if (hash == lanDeliveredSmsHash) {
                Log.i(VortexStack.TAG, "sms unchanged since LAN bulk delivery; skipping BLE burst")
                return@collect
            }
            sendSms(json)
        }
    }
}

/** Read one page of a conversation (laptop's SMS infinite scroll) and send it
 *  back over BLE as SMS_THREAD frames. [argJson] =
 *  `{"thread":<id>,"address":"<num>","offset":<n>,"limit":<n>}`. Gated by the
 *  same SMS-mirror toggle as the recent-list sync. */
internal fun VortexStack.handleLoadThread(argJson: String) {
    val thread: Long
    val address: String
    val offset: Int
    val limit: Int
    try {
        val o = org.json.JSONObject(argJson)
        thread = o.optLong("thread", 0L)
        address = o.optString("address", "").trim()
        offset = o.optInt("offset", 0)
        limit = o.optInt("limit", 40)
    } catch (e: Exception) {
        Log.w(VortexStack.TAG, "load_thread: bad arg: ${e.message}")
        return
    }
    scope.launch {
        if (!com.vortex.a3.core.sms.SmsMirrorSetting.isEnabled()) return@launch
        val page = smsProvider?.loadThread(thread, address, offset, limit) ?: return@launch
        Log.i(VortexStack.TAG, "load_thread: thread=$thread offset=$offset → ${page.size} msg")
        sendSmsThread(com.vortex.a3.core.sms.smsToJsonBytes(page))
    }
}

/** Chunk + send a single conversation's page to every peer over BLE as
 *  SMS_THREAD frames (laptop merges them into the open thread).
 *  Payload per frame: [total u16 BE][idx u16 BE][json-chunk]. */
internal suspend fun VortexStack.sendSmsThread(json: ByteArray) = companionSendMutex.withLock {
    val total = ((json.size + CONTACTS_CHUNK - 1) / CONTACTS_CHUNK).coerceAtLeast(1)
    if (total > 0xFFFF) return@withLock
    for (peer in peerStore.list()) {
        val server = gattServer ?: return@withLock
        for (idx in 0 until total) {
            val start = idx * CONTACTS_CHUNK
            val end = minOf(start + CONTACTS_CHUNK, json.size)
            val payload = java.io.ByteArrayOutputStream().apply {
                write((total ushr 8) and 0xFF); write(total and 0xFF)
                write((idx ushr 8) and 0xFF); write(idx and 0xFF)
                write(json, start, end - start)
            }.toByteArray()
            if (!server.sendSmsThreadChunkEncrypted(peer.peerStaticPub, payload)) return@withLock
            kotlinx.coroutines.delay(20) // pace so the BLE stack doesn't drop notifies
        }
    }
    Log.i(VortexStack.TAG, "sms thread page sent ($total chunks, ${json.size} bytes)")
    kotlinx.coroutines.delay(800) // settle the BLE link before the next bulk burst
}

/** Chunk + send an SMS-list JSON blob to every peer over BLE.
 *  Payload per frame: [total u16 BE][idx u16 BE][json-chunk]. */
internal suspend fun VortexStack.sendSms(json: ByteArray) = companionSendMutex.withLock {
    val total = ((json.size + CONTACTS_CHUNK - 1) / CONTACTS_CHUNK).coerceAtLeast(1)
    if (total > 0xFFFF) return@withLock
    for (peer in peerStore.list()) {
        val server = gattServer ?: return@withLock
        for (idx in 0 until total) {
            val start = idx * CONTACTS_CHUNK
            val end = minOf(start + CONTACTS_CHUNK, json.size)
            val payload = java.io.ByteArrayOutputStream().apply {
                write((total ushr 8) and 0xFF); write(total and 0xFF)
                write((idx ushr 8) and 0xFF); write(idx and 0xFF)
                write(json, start, end - start)
            }.toByteArray()
            if (!server.sendSmsChunkEncrypted(peer.peerStaticPub, payload)) return@withLock
            kotlinx.coroutines.delay(20) // pace so the BLE stack doesn't drop notifies
        }
    }
    Log.i(VortexStack.TAG, "sms sent ($total chunks, ${json.size} bytes)")
    kotlinx.coroutines.delay(800) // settle the BLE link before the next bulk burst
}
