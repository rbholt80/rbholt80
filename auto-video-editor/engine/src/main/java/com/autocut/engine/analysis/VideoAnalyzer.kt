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
 * A run of frames that stopped changing at all: not a steady camera, which
 * still carries sensor noise and a talking subject's own movement, but no new
 * visual information arriving — a paused screen recording, a slide left on
 * screen, a capture that stalled. [motion] is a first difference between
 * consecutive frames, so this is measured directly from it rather than
 * inferred from sharpness or exposure.
 */
data class StaticSpan(val range: TimeRange)

/** Which way a frame has nothing usable on it. */
enum class BlankKind {
    /** Lens covered, camera put away, pointed somewhere with no light. */
    DARK,

    /** Blown out to white — a lens flare, an overexposed slide, a stuck capture. */
    BRIGHT,
}

/** A run of frames with essentially nothing on them. */
data class BlankSpan(val range: TimeRange, val kind: BlankKind)

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
    val staticSpans: List<StaticSpan>,
    val blankSpans: List<BlankSpan>,
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
            staticSpans = emptyList(),
            blankSpans = emptyList(),
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

    /**
     * Motion this low, sustained, means nothing new arrived between frames —
     * not a steady shot, which still carries sensor noise and a subject's own
     * small movement, but an actual freeze. Comfortably below the motion a
     * genuinely still camera reports (compare [STILL_MOTION_CEILING], which is
     * ten times looser and answers a different question: "is the camera still
     * enough to trust a sharpness reading", not "did the picture stop").
     */
    private const val FROZEN_MOTION_CEILING = 0.0015f
    private const val MIN_STATIC_SPAN_US = 2_500_000L

    /** Share of the clip a freeze may cover before it reads as intentional. */
    private const val STATIC_AUTO_LIMIT = 0.5f

    /** Almost every pixel crushed or blown, sustained: nothing is on screen. */
    private const val BLANK_SHADOW_RATIO = 0.97f
    private const val BLANK_HIGHLIGHT_RATIO = 0.97f
    private const val MIN_BLANK_SPAN_US = 1_500_000L

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

        val sharpness = FloatArray(frames.size) { frames[it].sharpness }
        val motion = FloatArray(frames.size) { frames[it].motion }
        val blankSpans = findBlankSpans(frames, signals.videoSampleIntervalUs)

        // A blank frame carries no photographic information, so it should not
        // pull exposure, colour or contrast judgements toward it — a lens-cap
        // stretch dragging the whole clip's target brightness down is exactly
        // the kind of thing that made an automatic edit look like it was
        // guessing. It is going to be cut anyway; the exposure stats should
        // describe the footage that survives, not the footage that does not.
        val contentFrames = frames.filter { classifyBlank(it) == null }.ifEmpty { frames }
        val luma = FloatArray(contentFrames.size) { contentFrames[it].meanLuma }
        val sortedLuma = luma.copyOf().also { it.sort() }
        val stdDev = FloatArray(contentFrames.size) { contentFrames[it].lumaStdDev }

        val exposure = ExposureProfile(
            medianLuma = Dsp.percentileOfSorted(sortedLuma, 0.5f),
            darkPercentileLuma = Dsp.percentileOfSorted(sortedLuma, 0.1f),
            brightPercentileLuma = Dsp.percentileOfSorted(sortedLuma, 0.9f),
            medianStdDev = Dsp.median(stdDev),
            shadowRatio = Dsp.mean(FloatArray(contentFrames.size) { contentFrames[it].shadowRatio }),
            highlightRatio = Dsp.mean(FloatArray(contentFrames.size) { contentFrames[it].highlightRatio }),
            meanU = Dsp.mean(FloatArray(contentFrames.size) { contentFrames[it].meanU }),
            meanV = Dsp.mean(FloatArray(contentFrames.size) { contentFrames[it].meanV }),
            meanChroma = Dsp.mean(FloatArray(contentFrames.size) { contentFrames[it].meanChroma }),
        )

        // Scene-change and camera-path work stays on every sampled frame,
        // blank ones included: skipping frames here would break the uniform
        // per-sample step the shake integration depends on, and a hard cut
        // into or out of a blank frame is still a hard cut.
        val allLuma = FloatArray(frames.size) { frames[it].meanLuma }

        return VideoProfile(
            exposure = exposure,
            focusSpans = findSoftSpans(frames, sharpness, motion, signals.videoSampleIntervalUs),
            staticSpans = findStaticSpans(frames, motion, signals.videoSampleIntervalUs),
            blankSpans = blankSpans,
            shake = solveShake(signals),
            sceneChanges = countSceneChanges(allLuma, motion),
            meanSharpness = Dsp.mean(sharpness),
        )
    }

    /** Which way, if any, [sample] has nothing usable on it. */
    private fun classifyBlank(sample: VideoSample): BlankKind? = when {
        sample.shadowRatio >= BLANK_SHADOW_RATIO -> BlankKind.DARK
        sample.highlightRatio >= BLANK_HIGHLIGHT_RATIO -> BlankKind.BRIGHT
        else -> null
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

    /**
     * Runs of frames the picture stopped changing across at all.
     *
     * The first sampled frame is skipped deliberately: [FrameProfiler] always
     * reports zero motion for it, since there is no previous frame to diff
     * against, and that zero is an artefact of where sampling started, not
     * evidence the picture had already frozen.
     */
    private fun findStaticSpans(
        frames: List<VideoSample>,
        motion: FloatArray,
        sampleIntervalUs: Long,
    ): List<StaticSpan> {
        if (frames.size < 4 || sampleIntervalUs <= 0L) return emptyList()

        val spans = ArrayList<StaticSpan>()
        var startIndex = -1

        fun close(endIndex: Int) {
            if (startIndex < 0) return
            val startUs = frames[startIndex].timeUs
            val endUs = frames[endIndex].timeUs + sampleIntervalUs
            if (endUs - startUs >= MIN_STATIC_SPAN_US) spans.add(StaticSpan(TimeRange(startUs, endUs)))
            startIndex = -1
        }

        for (i in 1 until frames.size) {
            if (motion[i] < FROZEN_MOTION_CEILING) {
                if (startIndex < 0) startIndex = i
            } else {
                close(i - 1)
            }
        }
        close(frames.lastIndex)
        return spans
    }

    /** Runs of frames with essentially nothing usable on them. */
    private fun findBlankSpans(frames: List<VideoSample>, sampleIntervalUs: Long): List<BlankSpan> {
        if (frames.isEmpty() || sampleIntervalUs <= 0L) return emptyList()

        val spans = ArrayList<BlankSpan>()
        var startIndex = -1
        var kind: BlankKind? = null

        fun close(endIndex: Int) {
            val runKind = kind ?: return
            if (startIndex < 0) return
            val startUs = frames[startIndex].timeUs
            val endUs = frames[endIndex].timeUs + sampleIntervalUs
            if (endUs - startUs >= MIN_BLANK_SPAN_US) {
                spans.add(BlankSpan(TimeRange(startUs, endUs), runKind))
            }
            startIndex = -1
            kind = null
        }

        for (i in frames.indices) {
            val frameKind = classifyBlank(frames[i])
            when {
                frameKind == null -> close(i - 1)
                frameKind != kind -> {
                    // A dark run ending exactly where a bright one begins is
                    // rare and not really one run, so it closes the old span
                    // before opening the new one rather than merging them.
                    close(i - 1)
                    startIndex = i
                    kind = frameKind
                }
                // else: same kind continues, nothing to record yet.
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
