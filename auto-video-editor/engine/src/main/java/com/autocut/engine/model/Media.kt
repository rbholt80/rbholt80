package com.autocut.engine.model

/**
 * What the container says about the source file, before a single frame is decoded.
 */
data class MediaProbe(
    val durationUs: Long,
    val width: Int,
    val height: Int,
    val frameRate: Float,
    /** Container rotation metadata: 0, 90, 180 or 270. */
    val rotationDegrees: Int = 0,
    val hasAudio: Boolean = true,
    val bitRate: Long = 0L,
    val mimeType: String? = null,
) {
    /** Width/height after the container's rotation is honoured, i.e. what a player shows. */
    val displayWidth: Int get() = if (rotationDegrees % 180 == 90) height else width
    val displayHeight: Int get() = if (rotationDegrees % 180 == 90) width else height

    val isPortrait: Boolean get() = displayHeight > displayWidth
}

/**
 * One short window of audio, already reduced to two numbers.
 *
 * The extractor decodes PCM and collapses each window down to these before the
 * engine ever sees it — a minute of audio becomes a few thousand samples rather
 * than millions, which is what makes the planner cheap enough to re-run on every
 * toggle in the UI.
 *
 * @param rms  linear root-mean-square amplitude of the window, 0f..1f
 * @param peak largest absolute sample in the window, 0f..1f
 */
data class AudioSample(
    val startUs: Long,
    val durationUs: Long,
    val rms: Float,
    val peak: Float,
) {
    val endUs: Long get() = startUs + durationUs
    val midUs: Long get() = startUs + durationUs / 2
}

/**
 * One decoded video frame, reduced to statistics over a downscaled greyscale copy.
 *
 * [shiftX]/[shiftY] are the estimated global translation from the *previous*
 * sampled frame, measured in analysis-frame pixels (see
 * [MediaSignals.analysisWidth]). They are what stabilisation is built from.
 */
data class VideoSample(
    val timeUs: Long,
    /** Mean luma, 0f..255f. */
    val meanLuma: Float,
    /** Standard deviation of luma, 0f..~128f. Low means flat/hazy, high means contrasty. */
    val lumaStdDev: Float,
    /** Fraction of pixels crushed to black. */
    val shadowRatio: Float,
    /** Fraction of pixels blown to white. */
    val highlightRatio: Float,
    /** Mean Cb, 0f..255f. 128f is neutral; above means blue-shifted. */
    val meanU: Float = NEUTRAL_CHROMA,
    /** Mean Cr, 0f..255f. 128f is neutral; above means red-shifted. */
    val meanV: Float = NEUTRAL_CHROMA,
    /**
     * Mean distance of each pixel's chroma from neutral, 0f..1f.
     *
     * Kept separate from [meanU]/[meanV] because they average a colourful scene
     * back to neutral: a frame that is half red and half cyan has neutral means
     * but is not washed out. This is what saturation decisions read.
     */
    val meanChroma: Float = 0f,
    /** Variance of the Laplacian over the downscaled frame. Higher is sharper. */
    val sharpness: Float = 0f,
    /** Mean absolute luma difference from the previous sampled frame, normalised 0f..1f. */
    val motion: Float = 0f,
    val shiftX: Float = 0f,
    val shiftY: Float = 0f,
) {
    companion object {
        const val NEUTRAL_CHROMA = 128f
    }
}

/**
 * Everything the planner is allowed to look at.
 *
 * [analysisWidth]/[analysisHeight] describe the downscaled frame the video
 * statistics were computed on, so shifts measured in those pixels can be turned
 * back into a fraction of the real frame.
 */
data class MediaSignals(
    val probe: MediaProbe,
    val audio: List<AudioSample>,
    val video: List<VideoSample>,
    val analysisWidth: Int = 96,
    val analysisHeight: Int = 54,
) {
    val hasAudioSignal: Boolean get() = probe.hasAudio && audio.isNotEmpty()
    val hasVideoSignal: Boolean get() = video.isNotEmpty()

    /** Seconds between sampled video frames, or 0 when there is nothing to measure. */
    val videoSampleIntervalUs: Long
        get() = if (video.size < 2) 0L else (video.last().timeUs - video.first().timeUs) / (video.size - 1)
}

/**
 * A half-open span of source time, `[startUs, endUs)`.
 */
data class TimeRange(val startUs: Long, val endUs: Long) : Comparable<TimeRange> {

    val durationUs: Long get() = (endUs - startUs).coerceAtLeast(0L)
    val isEmpty: Boolean get() = endUs <= startUs

    operator fun contains(timeUs: Long): Boolean = timeUs >= startUs && timeUs < endUs

    fun overlaps(other: TimeRange): Boolean = startUs < other.endUs && other.startUs < endUs

    fun clampedTo(bounds: TimeRange): TimeRange =
        TimeRange(startUs.coerceIn(bounds.startUs, bounds.endUs), endUs.coerceIn(bounds.startUs, bounds.endUs))

    override fun compareTo(other: TimeRange): Int =
        if (startUs != other.startUs) startUs.compareTo(other.startUs) else endUs.compareTo(other.endUs)

    companion object {
        /** Sorts and unions overlapping or touching ranges. Empty ranges are dropped. */
        fun merge(ranges: List<TimeRange>): List<TimeRange> {
            val sorted = ranges.filterNot { it.isEmpty }.sorted()
            if (sorted.isEmpty()) return emptyList()
            val out = ArrayList<TimeRange>(sorted.size)
            var current = sorted.first()
            for (i in 1 until sorted.size) {
                val next = sorted[i]
                current = if (next.startUs <= current.endUs) {
                    TimeRange(current.startUs, maxOf(current.endUs, next.endUs))
                } else {
                    out.add(current)
                    next
                }
            }
            out.add(current)
            return out
        }

        /** The parts of [bounds] not covered by [ranges]. */
        fun complement(bounds: TimeRange, ranges: List<TimeRange>): List<TimeRange> {
            val merged = merge(ranges.map { it.clampedTo(bounds) })
            val out = ArrayList<TimeRange>(merged.size + 1)
            var cursor = bounds.startUs
            for (range in merged) {
                if (range.startUs > cursor) out.add(TimeRange(cursor, range.startUs))
                cursor = maxOf(cursor, range.endUs)
            }
            if (cursor < bounds.endUs) out.add(TimeRange(cursor, bounds.endUs))
            return out
        }

        fun totalDurationUs(ranges: List<TimeRange>): Long = merge(ranges).sumOf { it.durationUs }
    }
}
