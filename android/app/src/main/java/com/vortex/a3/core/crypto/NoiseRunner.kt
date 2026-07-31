package com.vortex.a3.core.crypto

import com.southernstorm.noise.protocol.HandshakeState
import com.southernstorm.noise.protocol.Noise

/** Noise XX / IK runners per spec §6 and §7. */
object NoiseRunner {
    const val NOISE_XX = "Noise_XX_25519_ChaChaPoly_SHA256"
    /** The reconnect pattern — used for the test vectors AND at runtime. At
     *  runtime the Pairwise Reconnect Secret is mixed into the prologue (see
     *  ReconnectOrchestrator.buildResponder and LanServer), which is what
     *  keeps a reconnect authenticated after a long-term static-key
     *  compromise — the goal Noise_IKpsk2 would serve, without needing a
     *  pattern this Noise library does not implement. */
    const val NOISE_IK = "Noise_IK_25519_ChaChaPoly_SHA256"

    val PROLOGUE_XX: ByteArray = "vortex/v1/pairing".toByteArray(Charsets.UTF_8)
    val PROLOGUE_IK: ByteArray = "vortex/v1/reconnect".toByteArray(Charsets.UTF_8)

    data class Result(
        val messages: List<ByteArray>,
        val initiatorHandshakeHash: ByteArray,
        val responderHandshakeHash: ByteArray,
    )

    fun runXxDeterministic(
        initiatorStaticPriv: ByteArray,
        responderStaticPriv: ByteArray,
        initiatorEphemeralPriv: ByteArray,
        responderEphemeralPriv: ByteArray,
    ): Result = runNoise(
        pattern = NOISE_XX,
        prologue = PROLOGUE_XX,
        initiatorStaticPriv = initiatorStaticPriv,
        responderStaticPriv = responderStaticPriv,
        initiatorEphemeralPriv = initiatorEphemeralPriv,
        responderEphemeralPriv = responderEphemeralPriv,
        responderStaticPub = null,
    )

    fun runIkDeterministic(
        initiatorStaticPriv: ByteArray,
        responderStaticPriv: ByteArray,
        initiatorEphemeralPriv: ByteArray,
        responderEphemeralPriv: ByteArray,
        responderStaticPub: ByteArray,
    ): Result = runNoise(
        pattern = NOISE_IK,
        prologue = PROLOGUE_IK,
        initiatorStaticPriv = initiatorStaticPriv,
        responderStaticPriv = responderStaticPriv,
        initiatorEphemeralPriv = initiatorEphemeralPriv,
        responderEphemeralPriv = responderEphemeralPriv,
        responderStaticPub = responderStaticPub,
    )

    private fun runNoise(
        pattern: String,
        prologue: ByteArray,
        initiatorStaticPriv: ByteArray,
        responderStaticPriv: ByteArray,
        initiatorEphemeralPriv: ByteArray,
        responderEphemeralPriv: ByteArray,
        responderStaticPub: ByteArray?,
    ): Result {
        val initiator = HandshakeState(pattern, HandshakeState.INITIATOR).apply {
            setPrologue(prologue, 0, prologue.size)
            localKeyPair.setPrivateKey(initiatorStaticPriv, 0)
            fixedEphemeralKey.setPrivateKey(initiatorEphemeralPriv, 0)
            responderStaticPub?.let { remotePublicKey.setPublicKey(it, 0) }
            start()
        }
        val responder = HandshakeState(pattern, HandshakeState.RESPONDER).apply {
            setPrologue(prologue, 0, prologue.size)
            localKeyPair.setPrivateKey(responderStaticPriv, 0)
            fixedEphemeralKey.setPrivateKey(responderEphemeralPriv, 0)
            start()
        }

        val messages = mutableListOf<ByteArray>()

        messages += writeOne(initiator).also { readOne(responder, it) }
        messages += writeOne(responder).also { readOne(initiator, it) }
        if (initiator.action == HandshakeState.WRITE_MESSAGE) {
            messages += writeOne(initiator).also { readOne(responder, it) }
        }

        return Result(
            messages = messages.toList(),
            initiatorHandshakeHash = initiator.handshakeHash.copyOf(),
            responderHandshakeHash = responder.handshakeHash.copyOf(),
        )
    }

    private fun writeOne(state: HandshakeState): ByteArray {
        val buf = ByteArray(Noise.MAX_PACKET_LEN)
        val len = state.writeMessage(buf, 0, null, 0, 0)
        return buf.copyOf(len)
    }

    private fun readOne(state: HandshakeState, msg: ByteArray) {
        val payload = ByteArray(Noise.MAX_PACKET_LEN)
        state.readMessage(msg, 0, msg.size, payload, 0)
    }
}
