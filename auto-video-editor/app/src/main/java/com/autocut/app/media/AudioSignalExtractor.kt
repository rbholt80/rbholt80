package com.autocut.app.media

import android.content.Context
import android.media.AudioFormat
import android.media.MediaCodec
import android.media.MediaExtractor
import android.media.MediaFormat
import android.net.Uri
import com.autocut.engine.model.AudioSample
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.ensureActive
import java.nio.ByteOrder
import kotlin.coroutines.coroutineContext
import kotlin.math.abs
import kotlin.math.sqrt

/**
 * Decodes the soundtrack and reduces it to one level reading per short window.
 *
 * A minute of stereo audio is about five million samples. Everything the planner
 * needs from it is the loudness envelope, so the PCM is collapsed to a level and
 * a peak per 100ms window as it streams past and never held in memory.
 */
class AudioSignalExtractor(
    private val context: Context,
    private val windowUs: Long = 100_000L,
) {

    /**
     * @param onProgress called with 0f..1f as decoding advances
     * @return one sample per window, or an empty list when the file has no audio
     */
    suspend fun extract(
        uri: Uri,
        durationUs: Long,
        onProgress: (Float) -> Unit = {},
    ): List<AudioSample> {
        val extractor = MediaExtractor()
        var codec: MediaCodec? = null
        try {
            extractor.setDataSource(context, uri, null)
            val trackIndex = extractor.findTrack("audio/") ?: return emptyList()
            extractor.selectTrack(trackIndex)

            val inputFormat = extractor.getTrackFormat(trackIndex)
            val mime = inputFormat.getString(MediaFormat.KEY_MIME) ?: return emptyList()
            codec = MediaCodec.createDecoderByType(mime).apply {
                configure(inputFormat, null, null, 0)
                start()
            }
            return decode(extractor, codec, durationUs, onProgress)
        } catch (e: MediaReadException) {
            throw e
        } catch (e: CancellationException) {
            // Cancellation is not a decode failure; the catch below would
            // otherwise rewrite it into "the soundtrack could not be decoded".
            throw e
        } catch (e: Exception) {
            // A missing or undecodable soundtrack is not a reason to give up on
            // the video: the planner simply works without sound.
            throw MediaReadException("The soundtrack could not be decoded.", e)
        } finally {
            runCatching { codec?.stop() }
            runCatching { codec?.release() }
            runCatching { extractor.release() }
        }
    }

    private suspend fun decode(
        extractor: MediaExtractor,
        codec: MediaCodec,
        durationUs: Long,
        onProgress: (Float) -> Unit,
    ): List<AudioSample> {
        val samples = ArrayList<AudioSample>()
        val bufferInfo = MediaCodec.BufferInfo()

        var sampleRate = 0
        var channelCount = 1
        var inputDone = false
        var outputDone = false
        var idleWaits = 0

        // Accumulators for the window currently being filled.
        var framesEmitted = 0L
        var framesInWindow = 0L
        var framesPerWindow = 0L
        var sumSquares = 0.0
        var peak = 0f
        var valuesInWindow = 0L

        fun flushWindow() {
            if (valuesInWindow == 0L || sampleRate <= 0) return
            val startUs = framesEmitted * 1_000_000L / sampleRate
            val lengthUs = framesInWindow * 1_000_000L / sampleRate
            samples.add(
                AudioSample(
                    startUs = startUs,
                    durationUs = lengthUs,
                    rms = sqrt(sumSquares / valuesInWindow).toFloat(),
                    peak = peak,
                )
            )
            framesEmitted += framesInWindow
            framesInWindow = 0
            valuesInWindow = 0
            sumSquares = 0.0
            peak = 0f
        }

        while (!outputDone) {
            coroutineContext.ensureActive()

            var progressed = false
            if (!inputDone) {
                val inputIndex = codec.dequeueInputBuffer(TIMEOUT_US)
                if (inputIndex >= 0) {
                    progressed = true
                    val buffer = codec.getInputBuffer(inputIndex)!!
                    val size = extractor.readSampleData(buffer, 0)
                    if (size < 0) {
                        codec.queueInputBuffer(inputIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                        inputDone = true
                    } else {
                        codec.queueInputBuffer(inputIndex, 0, size, extractor.sampleTime, 0)
                        extractor.advance()
                    }
                }
            }

            when (val outputIndex = codec.dequeueOutputBuffer(bufferInfo, TIMEOUT_US)) {
                MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    progressed = true
                    val format = codec.outputFormat
                    sampleRate = format.getInteger(MediaFormat.KEY_SAMPLE_RATE)
                    channelCount = format.getInteger(MediaFormat.KEY_CHANNEL_COUNT).coerceAtLeast(1)
                    framesPerWindow = (sampleRate * windowUs / 1_000_000L).coerceAtLeast(1L)
                    if (format.pcmEncoding() != AudioFormat.ENCODING_PCM_16BIT) {
                        throw MediaReadException("This soundtrack is in a PCM format the app cannot read.")
                    }
                }

                MediaCodec.INFO_TRY_AGAIN_LATER -> {
                    // Counts iterations that made no progress on either side.
                    // Keying this off "input is finished" missed the common
                    // hardware stall where the codec holds every input buffer,
                    // so input never finishes and the guard never armed.
                    if (!progressed && ++idleWaits > MAX_IDLE_WAITS) {
                        throw MediaReadException("The audio decoder stopped responding.")
                    }
                }

                else -> {
                    if (outputIndex < 0) continue
                    progressed = true
                    idleWaits = 0
                    val buffer = codec.getOutputBuffer(outputIndex)
                    if (buffer != null && bufferInfo.size > 0 && sampleRate > 0) {
                        // MediaCodec hands back PCM in the device's native byte
                        // order, but a ByteBuffer defaults to big endian. Without
                        // this the high and low bytes of every sample swap and
                        // the levels read as noise.
                        buffer.order(ByteOrder.LITTLE_ENDIAN)
                        buffer.position(bufferInfo.offset)
                        buffer.limit(bufferInfo.offset + bufferInfo.size)
                        var index = 0
                        val shortCount = bufferInfo.size / 2
                        while (index < shortCount) {
                            val value = buffer.short.toFloat() / Short.MAX_VALUE
                            sumSquares += value.toDouble() * value
                            valuesInWindow++
                            val magnitude = abs(value)
                            if (magnitude > peak) peak = magnitude
                            index++
                            if (index % channelCount == 0) {
                                framesInWindow++
                                if (framesInWindow >= framesPerWindow) flushWindow()
                            }
                        }
                        if (durationUs > 0) {
                            onProgress((bufferInfo.presentationTimeUs.toFloat() / durationUs).coerceIn(0f, 1f))
                        }
                    }
                    codec.releaseOutputBuffer(outputIndex, false)
                    if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) outputDone = true
                }
            }
        }

        flushWindow()
        onProgress(1f)
        return samples
    }

    private fun MediaFormat.pcmEncoding(): Int =
        if (containsKey(MediaFormat.KEY_PCM_ENCODING)) {
            getInteger(MediaFormat.KEY_PCM_ENCODING)
        } else {
            AudioFormat.ENCODING_PCM_16BIT
        }

    private companion object {
        const val TIMEOUT_US = 10_000L

        /** Ten seconds of a silent decoder before it is declared wedged. */
        const val MAX_IDLE_WAITS = 1_000
    }
}

/** Index of the first track whose mime type starts with [prefix], or null. */
internal fun MediaExtractor.findTrack(prefix: String): Int? {
    for (index in 0 until trackCount) {
        val mime = getTrackFormat(index).getString(MediaFormat.KEY_MIME) ?: continue
        if (mime.startsWith(prefix)) return index
    }
    return null
}
