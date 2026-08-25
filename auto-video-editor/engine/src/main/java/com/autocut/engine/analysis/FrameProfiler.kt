package com.autocut.engine.analysis

import com.autocut.engine.model.VideoSample
import kotlin.math.abs
import kotlin.math.sqrt

/**
 * Turns a stream of downscaled greyscale frames into [VideoSample]s.
 *
 * The platform side of the app owns the decoder and hands each frame here as a
 * small luma grid — 96x54 is plenty, and shrinking first is what makes
 * per-frame analysis of a long video affordable. Everything past that point is
 * arithmetic, so it lives in the engine where it can be tested against frames
 * whose true content is known.
 *
 * Not thread safe: it carries the previous frame so it can measure change, so
 * one instance belongs to one decode pass.
 */
class FrameProfiler(
    private val width: Int,
    private val height: Int,
    /**
     * Largest displacement the shift estimator will look for, in analysis
     * pixels. Eight out of 96 is about 8% of the frame between one frame and the
     * next — far more than a hand shakes, and past that a "shift" is a cut.
     */
    private val maxShiftPx: Int = 8,
) {
    init {
        require(width > 4 && height > 4) { "analysis frame too small: ${width}x$height" }
    }

    private var previousLuma: IntArray? = null
    private var previousColumns: FloatArray? = null
    private var previousRows: FloatArray? = null

    /** Forgets the previous frame, so the next one reports no motion or shift. */
    fun reset() {
        previousLuma = null
        previousColumns = null
        previousRows = null
    }

    /**
     * @param luma      row-major greyscale values 0..255, [width] * [height] long
     * @param meanU     mean Cb of the frame, 0f..255f
     * @param meanV     mean Cr of the frame, 0f..255f
     * @param meanChroma mean distance from neutral colour, 0f..1f
     */
    fun profile(
        timeUs: Long,
        luma: IntArray,
        meanU: Float = VideoSample.NEUTRAL_CHROMA,
        meanV: Float = VideoSample.NEUTRAL_CHROMA,
        meanChroma: Float = 0f,
    ): VideoSample {
        require(luma.size == width * height) {
            "expected ${width * height} luma values, got ${luma.size}"
        }

        var sum = 0.0
        var sumSquares = 0.0
        var shadow = 0
        var highlight = 0
        val columns = FloatArray(width)
        val rows = FloatArray(height)

        for (y in 0 until height) {
            val rowOffset = y * width
            var rowSum = 0.0
            for (x in 0 until width) {
                val value = luma[rowOffset + x]
                rowSum += value
                columns[x] += value.toFloat()
                if (value < SHADOW_LEVEL) shadow++
                if (value > HIGHLIGHT_LEVEL) highlight++
                sumSquares += value.toDouble() * value
            }
            rows[y] = (rowSum / width).toFloat()
            sum += rowSum
        }
        for (x in 0 until width) columns[x] /= height

        val pixels = (width * height).toDouble()
        val meanLuma = (sum / pixels).toFloat()
        val variance = (sumSquares / pixels) - meanLuma.toDouble() * meanLuma

        val previous = previousLuma
        val motion = if (previous == null) 0f else meanAbsoluteDifference(previous, luma)
        val shiftX = previousColumns?.let { estimateShift(it, columns) } ?: 0f
        val shiftY = previousRows?.let { estimateShift(it, rows) } ?: 0f

        previousLuma = luma.copyOf()
        previousColumns = columns
        previousRows = rows

        return VideoSample(
            timeUs = timeUs,
            meanLuma = meanLuma,
            lumaStdDev = sqrt(variance.coerceAtLeast(0.0)).toFloat(),
            shadowRatio = shadow / pixels.toFloat(),
            highlightRatio = highlight / pixels.toFloat(),
            meanU = meanU,
            meanV = meanV,
            meanChroma = meanChroma,
            sharpness = laplacianVariance(luma),
            motion = motion,
            shiftX = shiftX,
            shiftY = shiftY,
        )
    }

    private fun meanAbsoluteDifference(previous: IntArray, current: IntArray): Float {
        var total = 0L
        for (i in current.indices) total += abs(current[i] - previous[i]).toLong()
        return (total.toDouble() / current.size / 255.0).toFloat()
    }

    /**
     * Variance of the Laplacian — the standard cheap focus measure. A sharp
     * frame has strong second derivatives at its edges; a soft one does not.
     */
    private fun laplacianVariance(luma: IntArray): Float {
        if (width < 3 || height < 3) return 0f
        var sum = 0.0
        var sumSquares = 0.0
        var count = 0
        for (y in 1 until height - 1) {
            val row = y * width
            for (x in 1 until width - 1) {
                val index = row + x
                val response = 4 * luma[index] -
                    luma[index - 1] - luma[index + 1] -
                    luma[index - width] - luma[index + width]
                sum += response
                sumSquares += response.toDouble() * response
                count++
            }
        }
        if (count == 0) return 0f
        val mean = sum / count
        return ((sumSquares / count) - mean * mean).coerceAtLeast(0.0).toFloat()
    }

    /**
     * Estimates how far the picture moved along one axis, by matching the
     * frame's own projection profile against the previous frame's.
     *
     * Collapsing the frame to a row of column averages (and a column of row
     * averages) turns a two-dimensional search into two one-dimensional ones.
     * It costs a few thousand operations per frame instead of a few hundred
     * thousand, which is the difference between analysing every frame of a long
     * clip and having to skip most of them — and skipping frames is fatal for
     * stabilisation, which needs the whole camera path.
     *
     * It only sees translation. Rotation and zoom are invisible to it, which is
     * an accepted limit: hand shake is overwhelmingly translation.
     */
    private fun estimateShift(previous: FloatArray, current: FloatArray): Float {
        val span = current.size
        val limit = minOf(maxShiftPx, span / 4)
        if (limit < 1) return 0f

        // A profile with almost no variation along it — a clear sky, a blank
        // wall, a frame whose detail all runs the other way — cannot be matched.
        // Every offset scores about the same, so the winner is whichever noise
        // came out lowest. Reporting zero is not a missed measurement here, it
        // is the honest one: a confident wrong shift would be fed into the
        // camera path and stabilisation would then shove the frame around to
        // cancel movement that never happened.
        val contrast = Dsp.meanAbsoluteDelta(current)
        if (contrast < MIN_PROFILE_CONTRAST) return 0f

        var bestOffset = 0
        var bestCost = Float.MAX_VALUE
        val costs = FloatArray(limit * 2 + 1)

        for (offset in -limit..limit) {
            val from = maxOf(0, -offset)
            val to = minOf(span, span - offset)
            if (to - from < span / 2) {
                costs[offset + limit] = Float.MAX_VALUE
                continue
            }
            var total = 0.0
            for (i in from until to) total += abs(previous[i] - current[i + offset]).toDouble()
            val cost = (total / (to - from)).toFloat()
            costs[offset + limit] = cost
            if (cost < bestCost) {
                bestCost = cost
                bestOffset = offset
            }
        }

        // If even the best alignment leaves a residual far larger than the
        // profile's own variation, these two frames are not the same shot —
        // this is a cut, not a camera move. A cut reported as a huge shift
        // would put a step into the camera path that stabilisation spends the
        // next second trying to follow.
        if (bestCost > contrast * UNRELATED_FRAME_RATIO) return 0f

        // Parabola through the best match and its two neighbours, so the answer
        // is not quantised to whole analysis pixels. At 96 columns one pixel is
        // 1% of the frame, and quantising the camera path to that would itself
        // look like shake.
        val index = bestOffset + limit
        if (index <= 0 || index >= costs.size - 1) return bestOffset.toFloat()
        val left = costs[index - 1]
        val centre = costs[index]
        val right = costs[index + 1]
        if (left == Float.MAX_VALUE || right == Float.MAX_VALUE) return bestOffset.toFloat()

        val denominator = left - 2f * centre + right
        if (abs(denominator) < 1e-6f) return bestOffset.toFloat()
        val delta = (0.5f * (left - right) / denominator).coerceIn(-0.5f, 0.5f)
        return bestOffset + delta
    }

    private companion object {
        const val SHADOW_LEVEL = 24
        const val HIGHLIGHT_LEVEL = 235

        /** Grey levels of variation a projection needs before it can be matched. */
        const val MIN_PROFILE_CONTRAST = 1f

        /** Residual, as a multiple of that variation, past which it is a cut. */
        const val UNRELATED_FRAME_RATIO = 2f
    }
}
