package com.autocut.engine.analysis

import kotlin.math.abs
import kotlin.math.exp
import kotlin.math.log10
import kotlin.math.pow
import kotlin.math.roundToInt
import kotlin.math.sqrt

/**
 * The small pile of arithmetic every analyser needs.
 */
object Dsp {

    /** Anything quieter than this is reported as this, so logs of zero never appear. */
    const val SILENCE_FLOOR_DB = -96f

    fun amplitudeToDb(amplitude: Float): Float {
        val a = abs(amplitude)
        if (a <= 1e-5f) return SILENCE_FLOOR_DB
        return (20f * log10(a)).coerceAtLeast(SILENCE_FLOOR_DB)
    }

    fun dbToAmplitude(db: Float): Float = 10f.pow(db / 20f)

    /**
     * Linear-interpolated percentile. [fraction] is 0f..1f.
     */
    fun percentile(values: FloatArray, fraction: Float): Float {
        if (values.isEmpty()) return 0f
        val sorted = values.copyOf()
        sorted.sort()
        return percentileOfSorted(sorted, fraction)
    }

    fun percentileOfSorted(sorted: FloatArray, fraction: Float): Float {
        if (sorted.isEmpty()) return 0f
        if (sorted.size == 1) return sorted[0]
        val position = (fraction.coerceIn(0f, 1f) * (sorted.size - 1))
        val low = position.toInt()
        val high = (low + 1).coerceAtMost(sorted.size - 1)
        val t = position - low
        return sorted[low] + (sorted[high] - sorted[low]) * t
    }

    fun median(values: FloatArray): Float = percentile(values, 0.5f)

    fun mean(values: FloatArray): Float {
        if (values.isEmpty()) return 0f
        var sum = 0.0
        for (v in values) sum += v
        return (sum / values.size).toFloat()
    }

    fun rootMeanSquare(values: FloatArray): Float {
        if (values.isEmpty()) return 0f
        var sum = 0.0
        for (v in values) sum += v.toDouble() * v
        return sqrt(sum / values.size).toFloat()
    }

    /**
     * Gaussian smoothing with edge clamping, used to find the camera path a
     * steady hand would have taken.
     *
     * Edges are clamped rather than zero-padded: zero-padding would pull the
     * smoothed path toward the origin at the start and end of the clip, which
     * shows up as a lurch in the first and last second of a stabilised video.
     */
    fun gaussianSmooth(values: FloatArray, sigma: Float): FloatArray {
        if (values.size < 3 || sigma <= 0.01f) return values.copyOf()
        val radius = (sigma * 3f).roundToInt().coerceIn(1, values.size)
        val kernel = FloatArray(radius * 2 + 1)
        var kernelSum = 0f
        for (i in kernel.indices) {
            val x = (i - radius).toFloat()
            val w = exp(-(x * x) / (2f * sigma * sigma))
            kernel[i] = w
            kernelSum += w
        }
        for (i in kernel.indices) kernel[i] /= kernelSum

        val out = FloatArray(values.size)
        for (i in values.indices) {
            var acc = 0f
            for (k in kernel.indices) {
                val index = (i + k - radius).coerceIn(0, values.size - 1)
                acc += values[index] * kernel[k]
            }
            out[i] = acc
        }
        return out
    }

    /** Running sum of [values], with `out[i] = values[0] + ... + values[i]`. */
    fun cumulative(values: FloatArray): FloatArray {
        val out = FloatArray(values.size)
        var acc = 0f
        for (i in values.indices) {
            acc += values[i]
            out[i] = acc
        }
        return out
    }

    /** Mean absolute first difference — how jumpy a series is, ignoring its drift. */
    fun meanAbsoluteDelta(values: FloatArray): Float {
        if (values.size < 2) return 0f
        var sum = 0.0
        for (i in 1 until values.size) sum += abs(values[i] - values[i - 1]).toDouble()
        return (sum / (values.size - 1)).toFloat()
    }
}
