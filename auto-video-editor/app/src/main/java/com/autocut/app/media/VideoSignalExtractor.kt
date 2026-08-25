package com.autocut.app.media

import android.content.Context
import android.media.Image
import android.media.MediaCodec
import android.media.MediaCodecInfo
import android.media.MediaExtractor
import android.media.MediaFormat
import android.net.Uri
import com.autocut.engine.analysis.FrameProfiler
import com.autocut.engine.model.VideoSample
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.ensureActive
import kotlin.coroutines.coroutineContext
import kotlin.math.abs
import kotlin.math.max

/**
 * Decodes the picture and reduces every frame to a handful of numbers.
 *
 * Frames come back as YUV planes rather than as a Surface, because the analysis
 * needs to *read* pixels and a Surface only lets you draw them. Each frame is
 * point-sampled straight down to a [ANALYSIS_WIDTH] x [ANALYSIS_HEIGHT] grid
 * while it is being read, so the cost per frame is a few thousand byte loads no
 * matter whether the source is 720p or 4K.
 *
 * Every frame is looked at, not every tenth. Stabilisation needs the whole
 * camera path, and a path sampled twice a second is not a path.
 */
class VideoSignalExtractor(private val context: Context) {

    suspend fun extract(
        uri: Uri,
        durationUs: Long,
        onProgress: (Float) -> Unit = {},
    ): List<VideoSample> {
        val extractor = MediaExtractor()
        var codec: MediaCodec? = null
        try {
            extractor.setDataSource(context, uri, null)
            val trackIndex = extractor.findTrack("video/")
                ?: throw MediaReadException("This file has no video track.")
            extractor.selectTrack(trackIndex)

            val format = extractor.getTrackFormat(trackIndex)
            val mime = format.getString(MediaFormat.KEY_MIME)
                ?: throw MediaReadException("This file has no readable video format.")

            // Flexible YUV is the one colour format every decoder must be able
            // to produce, and the one getOutputImage knows how to describe.
            format.setInteger(
                MediaFormat.KEY_COLOR_FORMAT,
                MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible,
            )

            codec = MediaCodec.createDecoderByType(mime).apply {
                configure(format, null, null, 0)
                start()
            }
            return decode(extractor, codec, durationUs, onProgress)
        } catch (e: MediaReadException) {
            throw e
        } catch (e: CancellationException) {
            // Backing out of the screen is not a decode failure. Without this the
            // catch below would rewrite it into one and report it to the user.
            throw e
        } catch (e: Exception) {
            throw MediaReadException("The video could not be decoded.", e)
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
    ): List<VideoSample> {
        val samples = ArrayList<VideoSample>()
        val profiler = FrameProfiler(ANALYSIS_WIDTH, ANALYSIS_HEIGHT)
        val bufferInfo = MediaCodec.BufferInfo()
        val luma = IntArray(ANALYSIS_WIDTH * ANALYSIS_HEIGHT)

        // Very long recordings are thinned out rather than refused. The engine
        // works from timestamps, so a wider spacing simply means a coarser
        // camera path, not a wrong one.
        val stride = max(1, estimatedFrameCount(durationUs) / MAX_ANALYSIS_FRAMES)
        var frameIndex = 0
        var inputDone = false
        var outputDone = false
        var idleWaits = 0

        while (!outputDone) {
            // Decoding a long clip takes minutes. Without this, leaving the
            // screen or WorkManager stopping the job leaves a hardware decoder
            // and a worker thread running to the end of the file regardless.
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

            val outputIndex = codec.dequeueOutputBuffer(bufferInfo, TIMEOUT_US)
            if (outputIndex < 0) {
                // Count iterations that made no progress at all, on either side.
                // Keying this off "input is finished" missed the usual hardware
                // stall, where the codec holds every input buffer and returns
                // nothing: dequeueInputBuffer keeps failing so input never
                // finishes, the guard never arms, and the loop spins forever.
                if (!progressed && ++idleWaits > MAX_IDLE_WAITS) {
                    throw MediaReadException("The video decoder stopped responding.")
                }
            } else {
                idleWaits = 0
                val wanted = bufferInfo.size > 0 && frameIndex % stride == 0
                if (wanted) {
                    val image = runCatching { codec.getOutputImage(outputIndex) }.getOrNull()
                    if (image != null) {
                        try {
                            // COLOR_FormatYUV420Flexible is a request, not a
                            // promise. HDR sources — now the camera default on a
                            // lot of phones — commonly come back as 10-bit P010
                            // instead, where every sample is two bytes.
                            val highByte = if (image.planes[0].pixelStride >= 2) 1 else 0
                            readLuma(image, luma, highByte)
                            val chroma = readChroma(image, highByte)
                            samples.add(
                                profiler.profile(
                                    timeUs = bufferInfo.presentationTimeUs,
                                    luma = luma,
                                    meanU = chroma.meanU,
                                    meanV = chroma.meanV,
                                    meanChroma = chroma.meanChroma,
                                )
                            )
                        } finally {
                            // The image has to be released before its buffer is
                            // handed back to the decoder.
                            image.close()
                        }
                    }
                }
                if (bufferInfo.size > 0) frameIndex++
                codec.releaseOutputBuffer(outputIndex, false)

                if (durationUs > 0) {
                    onProgress((bufferInfo.presentationTimeUs.toFloat() / durationUs).coerceIn(0f, 1f))
                }
                if (bufferInfo.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) outputDone = true
            }
        }

        onProgress(1f)
        return samples
    }

    /**
     * Point-samples the luma plane down to the analysis grid.
     *
     * Nearest-neighbour rather than an averaging downscale: averaging would cost
     * a full read of every pixel in a 4K frame, and it would also smooth away
     * exactly the high-frequency detail the sharpness measure is looking for.
     *
     * [byteOffset] is 1 for 10-bit output. P010 stores each sample in the top
     * bits of a little-endian 16-bit word, so the high byte is an 8-bit version
     * of it; reading the low byte would return padding, and every measurement
     * downstream — exposure, sharpness, motion, the whole camera path — would be
     * computed from noise while looking perfectly plausible.
     */
    private fun readLuma(image: Image, out: IntArray, byteOffset: Int) {
        val crop = image.cropRect
        val plane = image.planes[0]
        val buffer = plane.buffer
        val rowStride = plane.rowStride
        val pixelStride = plane.pixelStride

        for (y in 0 until ANALYSIS_HEIGHT) {
            val sourceY = crop.top + y * crop.height() / ANALYSIS_HEIGHT
            val rowOffset = sourceY * rowStride
            val outOffset = y * ANALYSIS_WIDTH
            for (x in 0 until ANALYSIS_WIDTH) {
                val sourceX = crop.left + x * crop.width() / ANALYSIS_WIDTH
                out[outOffset + x] =
                    buffer.get(rowOffset + sourceX * pixelStride + byteOffset).toInt() and 0xFF
            }
        }
    }

    private class Chroma(val meanU: Float, val meanV: Float, val meanChroma: Float)

    /**
     * Averages the colour planes on a coarser grid than the luma.
     *
     * [Chroma.meanChroma] is kept separately from the means because the means
     * cancel out: a frame that is half red and half cyan averages to neutral but
     * is not washed out, and it is the washed-out case saturation decisions care
     * about.
     */
    private fun readChroma(image: Image, byteOffset: Int): Chroma {
        if (image.planes.size < 3) {
            return Chroma(VideoSample.NEUTRAL_CHROMA, VideoSample.NEUTRAL_CHROMA, 0f)
        }
        val crop = image.cropRect
        val uPlane = image.planes[1]
        val vPlane = image.planes[2]
        val chromaWidth = crop.width() / 2
        val chromaHeight = crop.height() / 2
        if (chromaWidth < CHROMA_GRID_WIDTH || chromaHeight < CHROMA_GRID_HEIGHT) {
            return Chroma(VideoSample.NEUTRAL_CHROMA, VideoSample.NEUTRAL_CHROMA, 0f)
        }

        var sumU = 0L
        var sumV = 0L
        var sumChroma = 0L
        var count = 0

        for (y in 0 until CHROMA_GRID_HEIGHT) {
            val sourceY = crop.top / 2 + y * chromaHeight / CHROMA_GRID_HEIGHT
            for (x in 0 until CHROMA_GRID_WIDTH) {
                val sourceX = crop.left / 2 + x * chromaWidth / CHROMA_GRID_WIDTH
                val u = uPlane.buffer
                    .get(sourceY * uPlane.rowStride + sourceX * uPlane.pixelStride + byteOffset)
                    .toInt() and 0xFF
                val v = vPlane.buffer
                    .get(sourceY * vPlane.rowStride + sourceX * vPlane.pixelStride + byteOffset)
                    .toInt() and 0xFF
                sumU += u
                sumV += v
                sumChroma += max(abs(u - 128), abs(v - 128)).toLong()
                count++
            }
        }
        if (count == 0) return Chroma(VideoSample.NEUTRAL_CHROMA, VideoSample.NEUTRAL_CHROMA, 0f)
        return Chroma(
            meanU = sumU.toFloat() / count,
            meanV = sumV.toFloat() / count,
            meanChroma = (sumChroma.toFloat() / count / 128f).coerceIn(0f, 1f),
        )
    }

    private fun estimatedFrameCount(durationUs: Long): Int =
        ((durationUs / 1_000_000.0) * 30.0).toInt().coerceAtLeast(1)

    private companion object {
        const val ANALYSIS_WIDTH = 96
        const val ANALYSIS_HEIGHT = 54
        const val CHROMA_GRID_WIDTH = 32
        const val CHROMA_GRID_HEIGHT = 18

        /** Roughly twenty minutes at 30fps before frames start being skipped. */
        const val MAX_ANALYSIS_FRAMES = 36_000

        const val TIMEOUT_US = 10_000L

        /** Ten seconds of a silent decoder before it is declared wedged. */
        const val MAX_IDLE_WAITS = 1_000
    }
}
