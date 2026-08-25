package com.autocut.engine

import com.autocut.engine.analysis.Dsp
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs

class DspTest {

    @Test
    fun `full scale is zero dBFS and half amplitude is about minus six`() {
        assertEquals(0f, Dsp.amplitudeToDb(1f), 0.01f)
        assertEquals(-6.02f, Dsp.amplitudeToDb(0.5f), 0.01f)
        assertEquals(-20f, Dsp.amplitudeToDb(0.1f), 0.01f)
    }

    @Test
    fun `silence reports the floor instead of negative infinity`() {
        assertEquals(Dsp.SILENCE_FLOOR_DB, Dsp.amplitudeToDb(0f), 0.001f)
        assertTrue(Dsp.amplitudeToDb(1e-12f).isFinite())
    }

    @Test
    fun `db round trips through amplitude`() {
        for (db in listOf(-60f, -30f, -12f, -1f)) {
            assertEquals(db, Dsp.amplitudeToDb(Dsp.dbToAmplitude(db)), 0.01f)
        }
    }

    @Test
    fun `percentile interpolates between neighbours`() {
        val values = floatArrayOf(0f, 10f, 20f, 30f, 40f)
        assertEquals(0f, Dsp.percentile(values, 0f), 0.001f)
        assertEquals(20f, Dsp.percentile(values, 0.5f), 0.001f)
        assertEquals(40f, Dsp.percentile(values, 1f), 0.001f)
        assertEquals(5f, Dsp.percentile(values, 0.125f), 0.001f)
    }

    @Test
    fun `percentile handles empty and single element input`() {
        assertEquals(0f, Dsp.percentile(FloatArray(0), 0.5f), 0.001f)
        assertEquals(7f, Dsp.percentile(floatArrayOf(7f), 0.9f), 0.001f)
    }

    @Test
    fun `gaussian smoothing removes alternating jitter but keeps the trend`() {
        // A steady ramp with a plus or minus 5 wobble on every other sample.
        val noisy = FloatArray(60) { it * 2f + if (it % 2 == 0) 5f else -5f }
        val smooth = Dsp.gaussianSmooth(noisy, sigma = 4f)

        val noisyJitter = Dsp.meanAbsoluteDelta(noisy)
        val smoothJitter = Dsp.meanAbsoluteDelta(smooth)
        assertTrue("smoothing should reduce jitter: $noisyJitter -> $smoothJitter", smoothJitter < noisyJitter / 4f)

        // The underlying slope of 2 per sample must survive in the middle.
        val slope = (smooth[45] - smooth[15]) / 30f
        assertEquals(2f, slope, 0.15f)
    }

    @Test
    fun `gaussian smoothing clamps at the edges rather than pulling toward zero`() {
        val constant = FloatArray(40) { 100f }
        val smooth = Dsp.gaussianSmooth(constant, sigma = 5f)
        for (value in smooth) {
            assertEquals(100f, value, 0.01f)
        }
    }

    @Test
    fun `cumulative sums in order`() {
        assertTrue(Dsp.cumulative(floatArrayOf(1f, 2f, 3f)).contentEquals(floatArrayOf(1f, 3f, 6f)))
    }

    @Test
    fun `mean absolute delta ignores constant offset`() {
        val a = floatArrayOf(0f, 5f, 0f, 5f)
        val b = FloatArray(4) { a[it] + 1000f }
        assertTrue(abs(Dsp.meanAbsoluteDelta(a) - Dsp.meanAbsoluteDelta(b)) < 0.001f)
    }

    @Test
    fun `root mean square of a constant is that constant`() {
        assertEquals(0.25f, Dsp.rootMeanSquare(FloatArray(10) { 0.25f }), 1e-5f)
    }
}
