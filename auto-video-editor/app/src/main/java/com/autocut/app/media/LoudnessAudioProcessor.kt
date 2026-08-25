package com.autocut.app.media

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import com.autocut.engine.analysis.Dsp
import com.autocut.engine.model.AudioAdjust
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.tanh

/**
 * Applies the plan's gain, and optionally holds the peaks down.
 *
 * The limiter is a `tanh` soft clipper rather than a look-ahead limiter. It is
 * honest about what it is: gain reaching the ceiling is rounded over smoothly
 * instead of being squared off, which sounds like a touch of compression rather
 * than the crackle of hard clipping. It cannot exceed the ceiling, because
 * `tanh` never reaches 1 — which is the property that makes the planner's
 * "raise the level and catch the peaks" decision safe.
 */
@UnstableApi
class LoudnessAudioProcessor(adjust: AudioAdjust) : BaseAudioProcessor() {

    private val gain: Float = Dsp.dbToAmplitude(adjust.gainDb)
    private val limiterEnabled: Boolean = adjust.limiterEnabled
    private val ceiling: Float = Dsp.dbToAmplitude(adjust.limiterCeilingDb)

    override fun onConfigure(inputAudioFormat: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        if (inputAudioFormat.encoding != C.ENCODING_PCM_16BIT) {
            throw AudioProcessor.UnhandledAudioFormatException(inputAudioFormat)
        }
        return inputAudioFormat
    }

    override fun queueInput(inputBuffer: ByteBuffer) {
        val size = inputBuffer.remaining()
        if (size == 0) return
        // Rounded down to whole 16-bit samples. Media3 only ever queues whole
        // frames, so this should always equal size; asking for an odd number of
        // bytes and then writing an even number would hand back an output buffer
        // whose limit disagreed with what was written.
        val usable = size and 1.inv()
        if (usable == 0) {
            inputBuffer.position(inputBuffer.limit())
            return
        }
        val output = replaceOutputBuffer(usable)
        // A duplicate, not the caller's buffer: Media3 documents the input as
        // read-only apart from its position, and it reuses that buffer across
        // processors. Setting the byte order on it would reach outside this
        // processor. The duplicate shares the bytes but has its own order,
        // position and limit.
        val input = inputBuffer.duplicate().order(ByteOrder.LITTLE_ENDIAN)
        output.order(ByteOrder.LITTLE_ENDIAN)

        while (input.remaining() >= 2) {
            var value = input.short.toFloat() / Short.MAX_VALUE * gain
            if (limiterEnabled) {
                value = ceiling * tanh(value / ceiling)
            }
            val clamped = (value * Short.MAX_VALUE)
                .coerceIn(Short.MIN_VALUE.toFloat(), Short.MAX_VALUE.toFloat())
            output.putShort(clamped.toInt().toShort())
        }
        inputBuffer.position(inputBuffer.limit())
        output.flip()
    }
}
