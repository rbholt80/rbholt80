package com.autocut.engine.plan

import com.autocut.engine.model.Clip
import com.autocut.engine.model.TimeRange

/**
 * A stretch of source the plan wants played faster than real time.
 */
data class SpeedRamp(val range: TimeRange, val speed: Float) {
    init { require(speed > 0f) { "speed must be positive, was $speed" } }
}

/**
 * Turns "delete these spans, speed up those spans" into the ordered clip list
 * the renderer consumes.
 */
object Timeline {

    /**
     * @param minClipUs clips shorter than this are absorbed into the surrounding
     *   cut. A two-frame clip is a glitch, not an edit, and encoders handle them
     *   badly enough to be worth losing the frames.
     */
    fun assemble(
        durationUs: Long,
        cuts: List<TimeRange>,
        ramps: List<SpeedRamp> = emptyList(),
        minClipUs: Long = 120_000L,
    ): List<Clip> {
        if (durationUs <= 0L) return emptyList()
        val bounds = TimeRange(0L, durationUs)
        val keeps = TimeRange.complement(bounds, cuts)
            .filter { it.durationUs >= minClipUs }

        if (keeps.isEmpty()) return emptyList()

        val orderedRamps = normalizeRamps(ramps, bounds)
        val clips = ArrayList<Clip>(keeps.size)
        for (keep in keeps) {
            clips.addAll(splitByRamps(keep, orderedRamps))
        }
        return clips
    }

    /** Sorts ramps and drops overlaps, keeping the earlier of any two that collide. */
    private fun normalizeRamps(ramps: List<SpeedRamp>, bounds: TimeRange): List<SpeedRamp> {
        val sorted = ramps
            .map { SpeedRamp(it.range.clampedTo(bounds), it.speed) }
            .filterNot { it.range.isEmpty }
            .sortedBy { it.range.startUs }
        val out = ArrayList<SpeedRamp>(sorted.size)
        var lastEnd = Long.MIN_VALUE
        for (ramp in sorted) {
            if (ramp.range.startUs >= lastEnd) {
                out.add(ramp)
                lastEnd = ramp.range.endUs
            }
        }
        return out
    }

    private fun splitByRamps(keep: TimeRange, ramps: List<SpeedRamp>): List<Clip> {
        val overlapping = ramps.filter { it.range.overlaps(keep) }
        if (overlapping.isEmpty()) return listOf(Clip(keep.startUs, keep.endUs))

        val out = ArrayList<Clip>(overlapping.size * 2 + 1)
        var cursor = keep.startUs
        for (ramp in overlapping) {
            val rampStart = maxOf(ramp.range.startUs, keep.startUs)
            val rampEnd = minOf(ramp.range.endUs, keep.endUs)
            if (rampStart > cursor) out.add(Clip(cursor, rampStart))
            if (rampEnd > rampStart) out.add(Clip(rampStart, rampEnd, ramp.speed))
            cursor = maxOf(cursor, rampEnd)
        }
        if (cursor < keep.endUs) out.add(Clip(cursor, keep.endUs))
        return out
    }
}
