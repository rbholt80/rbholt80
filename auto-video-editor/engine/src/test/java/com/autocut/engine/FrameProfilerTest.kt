package com.autocut.engine

import com.autocut.engine.analysis.FrameProfiler
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.roundToInt
import kotlin.math.sin

class FrameProfilerTest {

    private val width = 96
    private val height = 54

    /**
     * A frame with independent texture along each axis, translated by (dx, dy).
     *
     * Additive rather than multiplicative on purpose: a separable product like
     * `sin(x) * sin(y)` averages away to nearly nothing along both projections,
     * which is the degenerate case covered by its own test below.
     */
    private fun texture(dx: Int = 0, dy: Int = 0): IntArray = IntArray(width * height) { index ->
        val x = index % width - dx
        val y = index / width - dy
        val value = 128 + 60 * sin(x * 0.55) + 50 * sin(y * 0.37)
        value.roundToInt().coerceIn(0, 255)
    }

    /** Texture that vanishes from both projection profiles when averaged. */
    private fun separableTexture(dx: Int = 0): IntArray = IntArray(width * height) { index ->
        val x = index % width - dx
        val y = index / width
        (128 + 90 * sin(x * 0.55) * sin(y * 0.4)).roundToInt().coerceIn(0, 255)
    }

    private fun flat(level: Int): IntArray = IntArray(width * height) { level }

    private fun profiler() = FrameProfiler(width, height)

    @Test
    fun `a mid grey frame measures as mid grey with no spread`() {
        val sample = profiler().profile(0L, flat(128))

        assertEquals(128f, sample.meanLuma, 0.01f)
        assertEquals(0f, sample.lumaStdDev, 0.01f)
        assertEquals(0f, sample.shadowRatio, 0.001f)
        assertEquals(0f, sample.highlightRatio, 0.001f)
    }

    @Test
    fun `crushed and blown pixels are counted`() {
        val frame = IntArray(width * height) { if (it % 4 == 0) 0 else if (it % 4 == 1) 255 else 128 }
        val sample = profiler().profile(0L, frame)

        assertEquals(0.25f, sample.shadowRatio, 0.01f)
        assertEquals(0.25f, sample.highlightRatio, 0.01f)
    }

    @Test
    fun `a textured frame reads as sharper than a flat one`() {
        val sharp = profiler().profile(0L, texture())
        val flat = profiler().profile(0L, flat(128))

        assertTrue(sharp.sharpness > 100f)
        assertEquals(0f, flat.sharpness, 0.01f)
    }

    @Test
    fun `the first frame reports no motion and no shift`() {
        val sample = profiler().profile(0L, texture())

        assertEquals(0f, sample.motion, 0.0001f)
        assertEquals(0f, sample.shiftX, 0.0001f)
        assertEquals(0f, sample.shiftY, 0.0001f)
    }

    @Test
    fun `a still camera reports no motion`() {
        val profiler = profiler()
        profiler.profile(0L, texture())
        val second = profiler.profile(100_000L, texture())

        assertEquals(0f, second.motion, 0.0001f)
        assertEquals(0f, second.shiftX, 0.05f)
        assertEquals(0f, second.shiftY, 0.05f)
    }

    @Test
    fun `a horizontal move is recovered with its direction`() {
        val profiler = profiler()
        profiler.profile(0L, texture())
        val moved = profiler.profile(100_000L, texture(dx = 3))

        assertEquals(3f, moved.shiftX, 0.35f)
        assertEquals(0f, moved.shiftY, 0.35f)
        assertTrue(moved.motion > 0f)
    }

    @Test
    fun `a vertical move is recovered on the other axis`() {
        val profiler = profiler()
        profiler.profile(0L, texture())
        val moved = profiler.profile(100_000L, texture(dy = -2))

        assertEquals(-2f, moved.shiftY, 0.35f)
        assertEquals(0f, moved.shiftX, 0.35f)
    }

    @Test
    fun `a diagonal move is recovered on both axes at once`() {
        val profiler = profiler()
        profiler.profile(0L, texture())
        val moved = profiler.profile(100_000L, texture(dx = -4, dy = 2))

        assertEquals(-4f, moved.shiftX, 0.35f)
        assertEquals(2f, moved.shiftY, 0.35f)
    }

    @Test
    fun `sub pixel movement is not rounded away`() {
        // A pure ramp shifted by half a pixel: the parabolic fit should land
        // between the whole-pixel candidates rather than on one of them.
        val ramp = { offset: Float ->
            IntArray(width * height) { index ->
                ((index % width) * 2f + offset).roundToInt().coerceIn(0, 255)
            }
        }
        val profiler = profiler()
        profiler.profile(0L, ramp(0f))
        val moved = profiler.profile(100_000L, ramp(1f))

        assertTrue("expected a fractional shift, got ${moved.shiftX}", moved.shiftX != 0f)
    }

    @Test
    fun `an axis with no structure reports no shift rather than a wrong one`() {
        // This texture leaves almost nothing in the row profile, so the vertical
        // match is guesswork. Guessing here would feed stabilisation a movement
        // that never happened.
        val profiler = profiler()
        profiler.profile(0L, separableTexture())
        val moved = profiler.profile(100_000L, separableTexture(dx = 3))

        assertEquals(0f, moved.shiftY, 0.0001f)
    }

    @Test
    fun `a cut is not reported as an enormous camera move`() {
        val profiler = profiler()
        profiler.profile(0L, texture())
        val afterCut = profiler.profile(100_000L, IntArray(width * height) { (it * 37) % 256 })

        assertEquals(0f, afterCut.shiftX, 0.0001f)
        assertEquals(0f, afterCut.shiftY, 0.0001f)
    }

    @Test
    fun `motion rises with how much of the frame changed`() {
        val small = profiler().let {
            it.profile(0L, texture())
            it.profile(100_000L, texture(dx = 1)).motion
        }
        val large = profiler().let {
            it.profile(0L, texture())
            it.profile(100_000L, flat(0)).motion
        }

        assertTrue("$small should be well under $large", small < large)
        assertTrue(large > 0.2f)
    }

    @Test
    fun `resetting forgets the previous frame`() {
        val profiler = profiler()
        profiler.profile(0L, texture())
        profiler.reset()
        val next = profiler.profile(100_000L, texture(dx = 5))

        assertEquals(0f, next.motion, 0.0001f)
        assertEquals(0f, next.shiftX, 0.0001f)
    }

    @Test
    fun `chroma is carried through untouched`() {
        val sample = profiler().profile(0L, flat(128), meanU = 100f, meanV = 150f, meanChroma = 0.3f)

        assertEquals(100f, sample.meanU, 0.001f)
        assertEquals(150f, sample.meanV, 0.001f)
        assertEquals(0.3f, sample.meanChroma, 0.001f)
    }

    @Test(expected = IllegalArgumentException::class)
    fun `a wrongly sized frame is rejected rather than read out of bounds`() {
        profiler().profile(0L, IntArray(10))
    }
}
