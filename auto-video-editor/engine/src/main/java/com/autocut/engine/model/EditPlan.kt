package com.autocut.engine.model

import java.util.Locale
import kotlin.math.abs
import kotlin.math.roundToLong

/**
 * One kept piece of the source, optionally played at a speed other than 1x.
 */
data class Clip(
    val sourceStartUs: Long,
    val sourceEndUs: Long,
    val speed: Float = 1f,
) {
    init {
        require(sourceEndUs >= sourceStartUs) { "clip ends before it starts: $sourceStartUs..$sourceEndUs" }
        require(speed > 0f) { "speed must be positive, was $speed" }
    }

    val sourceDurationUs: Long get() = sourceEndUs - sourceStartUs
    val outputDurationUs: Long get() = (sourceDurationUs / speed.toDouble()).roundToLong()
    val range: TimeRange get() = TimeRange(sourceStartUs, sourceEndUs)
}

/**
 * A translation to apply to one frame, expressed as a fraction of frame size so
 * it survives whatever resolution the renderer ends up working at.
 */
data class StabilizationKeyframe(
    val timeUs: Long,
    /** Positive moves the picture right, in fractions of frame width. */
    val offsetX: Float,
    /** Positive moves the picture up, in fractions of frame height. */
    val offsetY: Float,
)

/**
 * A shake-cancelling path plus the zoom needed to keep its edges off-screen.
 */
data class StabilizationTrack(
    val keyframes: List<StabilizationKeyframe>,
    val zoom: Float,
) {
    val isEmpty: Boolean get() = keyframes.isEmpty() || zoom <= 1.0001f

    /** Linearly interpolated offset at [timeUs]; clamps outside the keyframe range. */
    fun offsetAt(timeUs: Long): Pair<Float, Float> {
        if (keyframes.isEmpty()) return 0f to 0f
        if (timeUs <= keyframes.first().timeUs) return keyframes.first().let { it.offsetX to it.offsetY }
        if (timeUs >= keyframes.last().timeUs) return keyframes.last().let { it.offsetX to it.offsetY }
        // Keyframes are time-ordered, so a binary search finds the bracketing pair.
        var low = 0
        var high = keyframes.size - 1
        while (high - low > 1) {
            val mid = (low + high) ushr 1
            if (keyframes[mid].timeUs <= timeUs) low = mid else high = mid
        }
        val a = keyframes[low]
        val b = keyframes[high]
        val span = (b.timeUs - a.timeUs).toFloat()
        val t = if (span <= 0f) 0f else (timeUs - a.timeUs) / span
        return (a.offsetX + (b.offsetX - a.offsetX) * t) to (a.offsetY + (b.offsetY - a.offsetY) * t)
    }

    /**
     * The slice of this track covering [clip], with times rebased to the clip's
     * own timeline.
     *
     * The renderer sees each clip as a media item starting at zero, so a track
     * indexed by source time would apply the wrong correction to every clip
     * after the first cut. Endpoints are interpolated rather than dropped so a
     * clip that starts between two keyframes still begins at the right offset.
     */
    fun forClip(clip: Clip): StabilizationTrack {
        if (keyframes.isEmpty()) return this
        fun rebase(timeUs: Long): Long =
            ((timeUs - clip.sourceStartUs) / clip.speed.toDouble()).roundToLong().coerceAtLeast(0L)

        val out = ArrayList<StabilizationKeyframe>(keyframes.size + 2)
        val start = offsetAt(clip.sourceStartUs)
        out.add(StabilizationKeyframe(0L, start.first, start.second))
        for (frame in keyframes) {
            if (frame.timeUs <= clip.sourceStartUs || frame.timeUs >= clip.sourceEndUs) continue
            val rebased = rebase(frame.timeUs)
            if (rebased > out.last().timeUs) {
                out.add(StabilizationKeyframe(rebased, frame.offsetX, frame.offsetY))
            }
        }
        val end = offsetAt(clip.sourceEndUs)
        val endTime = rebase(clip.sourceEndUs)
        if (endTime > out.last().timeUs) out.add(StabilizationKeyframe(endTime, end.first, end.second))
        return StabilizationTrack(out, zoom)
    }
}

/**
 * Every picture correction the renderer should apply, in units the renderer can
 * hand straight to Media3's effect classes.
 */
data class VideoAdjust(
    /** Multiplicative per-channel gain. 1f is untouched. */
    val redScale: Float = 1f,
    val greenScale: Float = 1f,
    val blueScale: Float = 1f,
    /** -1f..1f, matching Media3's `Contrast`. */
    val contrast: Float = 0f,
    /** -100f..100f, matching Media3's `HslAdjustment`. */
    val saturationPercent: Float = 0f,
    val stabilization: StabilizationTrack? = null,
    /**
     * When set, the *short* edge of the output is capped at this many pixels.
     *
     * Short edge rather than height, because half the video on a phone is
     * portrait: capping height would turn a 1080x1920 clip into 607x1080.
     */
    val maxShortSidePx: Int = 0,
) {
    val hasColorWork: Boolean
        get() = abs(redScale - 1f) > 0.002f || abs(greenScale - 1f) > 0.002f ||
            abs(blueScale - 1f) > 0.002f || abs(contrast) > 0.002f || abs(saturationPercent) > 0.2f

    val hasGeometryWork: Boolean
        get() = (stabilization?.isEmpty == false) || maxShortSidePx > 0

    val isIdentity: Boolean get() = !hasColorWork && !hasGeometryWork
}

/**
 * Everything the renderer should do to the sound.
 */
data class AudioAdjust(
    val gainDb: Float = 0f,
    val limiterEnabled: Boolean = false,
    /** Ceiling the limiter clamps to, in dBFS. */
    val limiterCeilingDb: Float = -1f,
    val muted: Boolean = false,
) {
    val isIdentity: Boolean get() = !muted && !limiterEnabled && abs(gainDb) < 0.05f
}

/**
 * The finished decision: what to keep, what to change, and why.
 *
 * A plan is a pure function of the signals and the preferences that produced it,
 * so flipping a fix off and re-planning is cheap and always lands on the same
 * answer.
 */
data class EditPlan(
    val source: MediaProbe,
    val clips: List<Clip>,
    val video: VideoAdjust,
    val audio: AudioAdjust,
    /** Every candidate fix, enabled or not, in presentation order. */
    val fixes: List<Fix>,
    /** Things worth telling the user that the app did not or could not fix. */
    val notes: List<Note>,
    val style: EditStyle,
) {
    val outputDurationUs: Long get() = clips.sumOf { it.outputDurationUs }
    val removedDurationUs: Long get() = (source.durationUs - clips.sumOf { it.sourceDurationUs }).coerceAtLeast(0L)
    val enabledFixes: List<Fix> get() = fixes.filter { it.enabled }

    /** True when the plan would rewrite the file into something different. */
    val changesAnything: Boolean
        get() = enabledFixes.isNotEmpty() &&
            (clips.size != 1 || clips.first().sourceDurationUs != source.durationUs ||
                clips.first().speed != 1f || !video.isIdentity || !audio.isIdentity)

    fun fix(id: String): Fix? = fixes.firstOrNull { it.id == id }

    /** e.g. "1:04 -> 0:47, 17.2s trimmed". */
    fun summary(): String {
        val before = formatDuration(source.durationUs)
        val after = formatDuration(outputDurationUs)
        val saved = removedDurationUs
        return if (saved > 0) {
            String.format(Locale.ROOT, "%s -> %s, %.1fs trimmed", before, after, saved / 1_000_000.0)
        } else {
            "$before, no cuts"
        }
    }

    companion object {
        /** An explicit "leave it alone" plan, used for sources nothing can be done with. */
        fun untouched(source: MediaProbe, style: EditStyle, notes: List<Note> = emptyList()): EditPlan =
            EditPlan(
                source = source,
                clips = listOf(Clip(0L, source.durationUs)),
                video = VideoAdjust(),
                audio = AudioAdjust(),
                fixes = emptyList(),
                notes = notes,
                style = style,
            )

        fun formatDuration(us: Long): String {
            val totalSeconds = us / 1_000_000
            val minutes = totalSeconds / 60
            val seconds = totalSeconds % 60
            return if (minutes >= 60) {
                String.format(Locale.ROOT, "%d:%02d:%02d", minutes / 60, minutes % 60, seconds)
            } else {
                String.format(Locale.ROOT, "%d:%02d", minutes, seconds)
            }
        }
    }
}
