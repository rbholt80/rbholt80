package com.autocut.engine

import com.autocut.engine.model.AudioSample
import com.autocut.engine.model.MediaProbe
import com.autocut.engine.model.MediaSignals
import com.autocut.engine.model.TimeRange
import com.autocut.engine.model.VideoSample

/**
 * Synthetic signals, built to look like what the extractor produces on a real
 * file so the planner is exercised on the same shapes it will see on a phone.
 */
object Fixtures {

    const val WINDOW_US = 100_000L
    const val SPEECH_RMS = 0.1f      // -20 dBFS
    const val SPEECH_PEAK = 0.4f     // -8 dBFS
    const val ROOM_RMS = 0.001f      // -60 dBFS
    const val ROOM_PEAK = 0.004f

    fun probe(
        durationUs: Long,
        width: Int = 1920,
        height: Int = 1080,
        hasAudio: Boolean = true,
        frameRate: Float = 30f,
        rotationDegrees: Int = 0,
    ) = MediaProbe(
        durationUs = durationUs,
        width = width,
        height = height,
        frameRate = frameRate,
        rotationDegrees = rotationDegrees,
        hasAudio = hasAudio,
    )

    /**
     * Speech-level audio everywhere except inside [silences], which sit at room tone.
     */
    fun speechWithSilences(
        durationUs: Long,
        silences: List<TimeRange>,
        speechRms: Float = SPEECH_RMS,
        speechPeak: Float = SPEECH_PEAK,
        windowUs: Long = WINDOW_US,
    ): List<AudioSample> = buildList {
        var t = 0L
        while (t < durationUs) {
            val length = minOf(windowUs, durationUs - t)
            val mid = t + length / 2
            val quiet = silences.any { mid in it }
            add(
                AudioSample(
                    startUs = t,
                    durationUs = length,
                    rms = if (quiet) ROOM_RMS else speechRms,
                    peak = if (quiet) ROOM_PEAK else speechPeak,
                )
            )
            t += length
        }
    }

    /** Audio with no quiet parts at all — a music bed or a noisy street. */
    fun flatAudio(durationUs: Long, rms: Float = 0.08f, windowUs: Long = WINDOW_US): List<AudioSample> =
        speechWithSilences(durationUs, emptyList(), speechRms = rms, windowUs = windowUs)

    /**
     * A well-behaved picture: correctly exposed, neutral, sharp, locked off.
     */
    fun steadyVideo(
        durationUs: Long,
        intervalUs: Long = WINDOW_US,
        meanLuma: Float = 118f,
        lumaStdDev: Float = 52f,
        sharpness: Float = 100f,
        meanChroma: Float = 0.20f,
        motion: Float = 0.01f,
    ): List<VideoSample> = buildList {
        var t = 0L
        while (t < durationUs) {
            add(
                VideoSample(
                    timeUs = t,
                    meanLuma = meanLuma,
                    lumaStdDev = lumaStdDev,
                    shadowRatio = 0.02f,
                    highlightRatio = 0.02f,
                    sharpness = sharpness,
                    motion = motion,
                    meanChroma = meanChroma,
                )
            )
            t += intervalUs
        }
    }

    fun signals(
        durationUs: Long,
        audio: List<AudioSample> = speechWithSilences(durationUs, emptyList()),
        video: List<VideoSample> = steadyVideo(durationUs),
        probe: MediaProbe = probe(durationUs, hasAudio = audio.isNotEmpty()),
    ) = MediaSignals(probe = probe, audio = audio, video = video)

    fun seconds(value: Double): Long = (value * 1_000_000).toLong()
    fun range(startSeconds: Double, endSeconds: Double) = TimeRange(seconds(startSeconds), seconds(endSeconds))
}
