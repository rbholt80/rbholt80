package com.autocut.engine.analysis

import com.autocut.engine.model.MediaSignals
import com.autocut.engine.model.StabilizationKeyframe
import com.autocut.engine.model.StabilizationTrack
import com.autocut.engine.model.TimeRange
import com.autocut.engine.model.VideoSample
import kotlin.math.abs
import kotlin.math.max

/**
 * How the picture is exposed and coloured, across the whole clip.
 */
data class ExposureProfile(
    val medianLuma: Float,
    val darkPercentileLuma: Float,
    val brightPercentileLuma: Float,
    val medianStdDev: Float,
    val shadowRatio: Float,
    val highlightRatio: Float,
    val meanU: Float,
    val meanV: Float,
    /** Mean distance from neutral colour, 0f..1f. Low means washed out. */
    val meanChroma: Float,
) {
    /** Distance of the average colour from neutral grey, 0f..1f. */
    val colorCast: Float
        get() = (max(abs(meanU - VideoSample.NEUTRAL_CHROMA), abs(meanV - VideoSample.NEUTRAL_CHROMA)) / 128f)
            .coerceIn(0f, 1f)
}

/**
 * A stretch that is out of focus while nothing much is moving — the signature of
 * a camera hunting for focus or a shot nobody meant to keep. Motion blur during
 * a pan looks the same to a sharpness measure, which is why motion is part of
 * the test rather than sharpness alone.
 */
data class FocusSpan(
    val range: TimeRange,
    val relativeSharpness: Float,
)

/**
 * How much the camera wobbles, and the correction that would cancel it.
 */
data class ShakeProfile(
    /** Frame-to-frame jitter as a fraction of frame width. */
    val shakeIndex: Float,
    val meanMotion: Float,
    val track: StabilizationTrack?,
) {
    val isShaky: Boolean get() = shakeIndex >= SHAKY_THRESHOLD

    companion object {
        const val SHAKY_THRESHOLD = 0.006f
        val NONE = ShakeProfile(0f, 0f, null)
    }
}

data class VideoProfile(
    val exposure: ExposureProfile,
    val focusSpans: List<FocusSpan>,
    val shake: ShakeProfile,
    val sceneChanges: Int,
    val meanSharpness: Float,
) {
    companion object {
        val EMPTY = VideoProfile(
            exposure = ExposureProfile(
                medianLuma = 128f,
                darkPercentileLuma = 128f,
                brightPercentileLuma = 128f,
                medianStdDev = 50f,
                shadowRatio = 0f,
                highlightRatio = 0f,
                meanU = VideoSample.NEUTRAL_CHROMA,
                meanV = VideoSample.NEUTRAL_CHROMA,
                meanChroma = 0f,
            ),
            focusSpans = emptyList(),
            shake = ShakeProfile.NONE,
            sceneChanges = 0,
            meanSharpness = 0f,
        )
    }
}

/**
 * Turns sampled frame statistics into a description of the picture.
 */
object VideoAnalyzer {

    /** A frame is soft if it is this much blurrier than the clip's own median. */
    private const val SOFT_RATIO = 0.45f

    /** ...and only if the camera is near enough to still, so pans are not flagged. */
    private const val STILL_MOTION_CEILING = 0.035f

    private const val MIN_FOCUS_SPAN_US = 700_000L

    /** Luma jump between consecutive samples that reads as a cut or a new scene. */
    private const val SCENE_LUMA_JUMP = 22f
    private const val SCENE_MOTION_JUMP = 0.28f

    /** How far ahead and behind to look when working out the intended camera path. */
    private const val PATH_SMOOTHING_SECONDS = 0.7f

    /** Most a stabilised frame is allowed to be zoomed in to hide its edges. */
    private const val MAX_STABILIZATION_ZOOM = 1.16f

    /** Below this correction, stabilising costs a crop and buys nothing visible. */
    private const val MIN_USEFUL_CORRECTION = 0.004f

    fun analyze(signals: MediaSignals): VideoProfile {
        val frames = signals.video
        if (frames.isEmpty()) return VideoProfile.EMPTY

        val luma = FloatArray(frames.size) { frames[it].meanLuma }
        val sortedLuma = luma.copyOf().also { it.sort() }
        val stdDev = FloatArray(frames.size) { frames[it].lumaStdDev }
        val sharpness = FloatArray(frames.size) { frames[it].sharpness }
        val motion = FloatArray(frames.size) { frames[it].motion }

        val exposure = ExposureProfile(
            medianLuma = Dsp.percentileOfSorted(sortedLuma, 0.5f),
            darkPercentileLuma = Dsp.percentileOfSorted(sortedLuma, 0.1f),
            brightPercentileLuma = Dsp.percentileOfSorted(sortedLuma, 0.9f),
            medianStdDev = Dsp.median(stdDev),
            shadowRatio = Dsp.mean(FloatArray(frames.size) { frames[it].shadowRatio }),
            highlightRatio = Dsp.mean(FloatArray(frames.size) { frames[it].highlightRatio }),
            meanU = Dsp.mean(FloatArray(frames.size) { frames[it].meanU }),
            meanV = Dsp.mean(FloatArray(frames.size) { frames[it].meanV }),
            meanChroma = Dsp.mean(FloatArray(frames.size) { frames[it].meanChroma }),
        )

        return VideoProfile(
            exposure = exposure,
            focusSpans = findSoftSpans(frames, sharpness, motion, signals.videoSampleIntervalUs),
            shake = solveShake(signals),
            sceneChanges = countSceneChanges(luma, motion),
            meanSharpness = Dsp.mean(sharpness),
        )
    }

    private fun findSoftSpans(
        frames: List<VideoSample>,
        sharpness: FloatArray,
        motion: FloatArray,
        sampleIntervalUs: Long,
    ): List<FocusSpan> {
        val medianSharpness = Dsp.median(sharpness)
        if (medianSharpness <= 1e-4f || frames.size < 4 || sampleIntervalUs <= 0L) return emptyList()

        val softCeiling = medianSharpness * SOFT_RATIO
        val meanMotion = Dsp.mean(motion)
        val motionCeiling = max(STILL_MOTION_CEILING, meanMotion * 0.7f)

        val spans = ArrayList<FocusSpan>()
        var startIndex = -1
        var sharpnessSum = 0.0

        fun close(endIndex: Int) {
            if (startIndex < 0) return
            val startUs = frames[startIndex].timeUs
            val endUs = frames[endIndex].timeUs + sampleIntervalUs
            if (endUs - startUs >= MIN_FOCUS_SPAN_US) {
                val count = endIndex - startIndex + 1
                spans.add(
                    FocusSpan(
                        range = TimeRange(startUs, endUs),
                        relativeSharpness = (sharpnessSum / count).toFloat() / medianSharpness,
                    )
                )
            }
            startIndex = -1
            sharpnessSum = 0.0
        }

        for (i in frames.indices) {
            val soft = sharpness[i] < softCeiling && motion[i] < motionCeiling
            if (soft) {
                if (startIndex < 0) startIndex = i
                sharpnessSum += sharpness[i].toDouble()
            } else {
                close(i - 1)
            }
        }
        close(frames.lastIndex)
        return spans
    }

    private fun countSceneChanges(luma: FloatArray, motion: FloatArray): Int {
        var count = 0
        for (i in 1 until luma.size) {
            if (abs(luma[i] - luma[i - 1]) > SCENE_LUMA_JUMP || motion[i] > SCENE_MOTION_JUMP) count++
        }
        return count
    }

    /**
     * Works out the camera path a steady hand would have taken and returns the
     * per-frame translation that pulls the real path onto it.
     *
     * The classic path-smoothing approach: integrate the frame-to-frame shifts
     * into an actual camera path, low-pass it, and translate every frame by the
     * difference. Deliberate pans survive because they are low-frequency and
     * stay in the smoothed path; hand tremor is high-frequency and gets removed.
     *
     * The correction is bounded by how far the frame can be pushed before its
     * empty edge appears, so the zoom needed to hide that edge is computed from
     * the correction rather than guessed — and if the shake is too big to hide
     * within [MAX_STABILIZATION_ZOOM], the correction is scaled down to fit
     * instead of the video being cropped into uselessness.
     */
    private fun solveShake(signals: MediaSignals): ShakeProfile {
        val frames = signals.video
        val intervalUs = signals.videoSampleIntervalUs
        if (frames.size < 8 || intervalUs <= 0L) return ShakeProfile.NONE

        val shiftX = FloatArray(frames.size) { frames[it].shiftX }
        val shiftY = FloatArray(frames.size) { frames[it].shiftY }
        val meanMotion = Dsp.mean(FloatArray(frames.size) { frames[it].motion })

        val width = signals.analysisWidth.toFloat().coerceAtLeast(1f)
        val height = signals.analysisHeight.toFloat().coerceAtLeast(1f)

        // Shifts are already a first difference of position, so their own
        // frame-to-frame change is the jerk that reads as shake to the eye.
        val shakeIndex = (Dsp.meanAbsoluteDelta(shiftX) / width + Dsp.meanAbsoluteDelta(shiftY) / height) / 2f
        if (shakeIndex <= 0f) return ShakeProfile(0f, meanMotion, null)

        val samplesPerSecond = 1_000_000f / intervalUs
        val sigma = (PATH_SMOOTHING_SECONDS * samplesPerSecond).coerceIn(1f, frames.size / 3f)

        val pathX = Dsp.cumulative(shiftX)
        val pathY = Dsp.cumulative(shiftY)
        val smoothX = Dsp.gaussianSmooth(pathX, sigma)
        val smoothY = Dsp.gaussianSmooth(pathY, sigma)

        val offsetX = FloatArray(frames.size) { (smoothX[it] - pathX[it]) / width }
        val offsetY = FloatArray(frames.size) { (smoothY[it] - pathY[it]) / height }

        var maxCorrection = 0f
        for (i in frames.indices) {
            maxCorrection = max(maxCorrection, max(abs(offsetX[i]), abs(offsetY[i])))
        }
        if (maxCorrection < MIN_USEFUL_CORRECTION) return ShakeProfile(shakeIndex, meanMotion, null)

        // Zoom has to cover the correction on both sides of the frame.
        var zoom = 1f + 2f * maxCorrection
        var scale = 1f
        if (zoom > MAX_STABILIZATION_ZOOM) {
            val affordable = (MAX_STABILIZATION_ZOOM - 1f) / 2f
            scale = affordable / maxCorrection
            zoom = MAX_STABILIZATION_ZOOM
        }

        val keyframes = frames.indices.map { i ->
            StabilizationKeyframe(
                timeUs = frames[i].timeUs,
                offsetX = offsetX[i] * scale,
                offsetY = offsetY[i] * scale,
            )
        }
        return ShakeProfile(shakeIndex, meanMotion, StabilizationTrack(keyframes, zoom))
    }
}
