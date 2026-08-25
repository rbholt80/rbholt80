package com.autocut.engine

import com.autocut.engine.Fixtures.seconds
import com.autocut.engine.analysis.Dsp
import com.autocut.engine.analysis.VideoAnalyzer
import com.autocut.engine.model.Clip
import com.autocut.engine.model.MediaSignals
import com.autocut.engine.model.StabilizationKeyframe
import com.autocut.engine.model.StabilizationTrack
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.abs

class StabilizationTest {

    private val analysisWidth = 96
    private val analysisHeight = 54
    private val intervalUs = 100_000L
    private val frameCount = 300

    /**
     * A deliberate pan of [panPerFrame] px per frame with a [jitter] px shake
     * riding on top of it — a handheld tracking shot.
     */
    private fun handheld(
        panPerFrame: Float,
        jitter: Float,
        verticalJitter: Float = 0f,
    ): MediaSignals {
        val frames = (0 until frameCount).map { i ->
            val sign = if (i % 2 == 0) 1f else -1f
            Fixtures.steadyVideo(seconds(0.1))[0].copy(
                timeUs = i * intervalUs,
                shiftX = panPerFrame + sign * jitter,
                shiftY = sign * verticalJitter,
                motion = 0.05f,
            )
        }
        return MediaSignals(
            probe = Fixtures.probe(frameCount * intervalUs),
            audio = emptyList(),
            video = frames,
            analysisWidth = analysisWidth,
            analysisHeight = analysisHeight,
        )
    }

    /** The camera path the correction actually produces, in analysis pixels. */
    private fun stabilizedPath(signals: MediaSignals, track: StabilizationTrack): FloatArray {
        val raw = Dsp.cumulative(FloatArray(signals.video.size) { signals.video[it].shiftX })
        return FloatArray(signals.video.size) { i ->
            raw[i] + track.keyframes[i].offsetX * analysisWidth
        }
    }

    @Test
    fun `a locked off camera is not stabilised`() {
        val signals = MediaSignals(
            probe = Fixtures.probe(seconds(30.0)),
            audio = emptyList(),
            video = Fixtures.steadyVideo(seconds(30.0)),
        )
        val shake = VideoAnalyzer.analyze(signals).shake

        assertFalse(shake.isShaky)
        assertNull(shake.track)
    }

    @Test
    fun `handheld shake is detected`() {
        val shake = VideoAnalyzer.analyze(handheld(panPerFrame = 0.5f, jitter = 2f)).shake

        assertTrue("shake index was ${shake.shakeIndex}", shake.isShaky)
        assertNotNull(shake.track)
    }

    @Test
    fun `the correction removes the wobble`() {
        val signals = handheld(panPerFrame = 0.5f, jitter = 2f)
        val track = VideoAnalyzer.analyze(signals).shake.track!!

        val raw = Dsp.cumulative(FloatArray(signals.video.size) { signals.video[it].shiftX })
        val corrected = stabilizedPath(signals, track)

        val rawJitter = Dsp.meanAbsoluteDelta(raw)
        val correctedJitter = Dsp.meanAbsoluteDelta(corrected)
        assertTrue(
            "correction should smooth the path: $rawJitter -> $correctedJitter",
            correctedJitter < rawJitter / 2f,
        )
    }

    @Test
    fun `the deliberate pan survives stabilisation`() {
        val signals = handheld(panPerFrame = 0.5f, jitter = 2f)
        val track = VideoAnalyzer.analyze(signals).shake.track!!
        val corrected = stabilizedPath(signals, track)

        // Measured across the middle, clear of the clamped ends: 100 frames of
        // pan at 0.5px each should still be 50px of movement.
        assertEquals(50f, corrected[200] - corrected[100], 3f)
    }

    @Test
    fun `the zoom is only as much as the correction needs`() {
        val track = VideoAnalyzer.analyze(handheld(panPerFrame = 0.5f, jitter = 2f)).shake.track!!

        assertTrue("zoom was ${track.zoom}", track.zoom > 1f)
        assertTrue("zoom was ${track.zoom}", track.zoom <= 1.16f)

        // Whatever the zoom is, it has to cover the largest push in both directions.
        val maxOffset = track.keyframes.maxOf { maxOf(abs(it.offsetX), abs(it.offsetY)) }
        assertTrue(
            "zoom ${track.zoom} does not cover offset $maxOffset",
            track.zoom >= 1f + 2f * maxOffset - 0.001f,
        )
    }

    @Test
    fun `violent shake is corrected as far as the crop allows rather than cropping to nothing`() {
        val track = VideoAnalyzer.analyze(handheld(panPerFrame = 0f, jitter = 40f)).shake.track!!

        assertEquals(1.16f, track.zoom, 0.001f)
        val maxOffset = track.keyframes.maxOf { abs(it.offsetX) }
        assertEquals(0.08f, maxOffset, 0.005f)
    }

    @Test
    fun `vertical wobble is corrected as well as horizontal`() {
        val signals = handheld(panPerFrame = 0f, jitter = 0f, verticalJitter = 2f)
        val track = VideoAnalyzer.analyze(signals).shake.track!!

        val raw = Dsp.cumulative(FloatArray(signals.video.size) { signals.video[it].shiftY })
        val corrected = FloatArray(signals.video.size) { i ->
            raw[i] + track.keyframes[i].offsetY * analysisHeight
        }

        val rawJitter = Dsp.meanAbsoluteDelta(raw)
        val correctedJitter = Dsp.meanAbsoluteDelta(corrected)
        assertTrue(
            "vertical correction should smooth the path: $rawJitter -> $correctedJitter",
            correctedJitter < rawJitter / 2f,
        )
    }

    @Test
    fun `a picture that has drifted down is corrected upward`() {
        // shiftY is measured in raster order — the analysis frame is read from
        // the top, so positive means the content moved DOWN. Cancelling that has
        // to be a NEGATIVE offsetY. A renderer whose coordinate space is y-up
        // must flip this once at its own boundary; applying it unflipped doubles
        // the wobble instead of removing it, and no test saw that because this
        // fixture used to hard-code shiftY to zero.
        val signals = handheld(panPerFrame = 0f, jitter = 0f, verticalJitter = 2f)
        val track = VideoAnalyzer.analyze(signals).shake.track!!
        val raw = Dsp.cumulative(FloatArray(signals.video.size) { signals.video[it].shiftY })

        val low = signals.video.size / 2      // even index: content sits lower
        val high = low + 1                    // odd index: content sits higher
        assertTrue("fixture assumption", raw[low] > raw[high])

        assertTrue(
            "content low in frame must be pulled up, got ${track.keyframes[low].offsetY}",
            track.keyframes[low].offsetY < 0f,
        )
        assertTrue(
            "content high in frame must be pushed down, got ${track.keyframes[high].offsetY}",
            track.keyframes[high].offsetY > 0f,
        )
    }

    // ---- rebasing onto a cut timeline --------------------------------------

    private val fourSecondTrack = StabilizationTrack(
        keyframes = listOf(
            StabilizationKeyframe(seconds(0.0), 0f, 0f),
            StabilizationKeyframe(seconds(1.0), 0.1f, 0f),
            StabilizationKeyframe(seconds(2.0), 0.2f, 0f),
            StabilizationKeyframe(seconds(3.0), 0.3f, 0f),
        ),
        zoom = 1.05f,
    )

    @Test
    fun `offsets interpolate between keyframes`() {
        assertEquals(0.15f, fourSecondTrack.offsetAt(seconds(1.5)).first, 0.001f)
        assertEquals(0f, fourSecondTrack.offsetAt(seconds(-5.0)).first, 0.001f)
        assertEquals(0.3f, fourSecondTrack.offsetAt(seconds(99.0)).first, 0.001f)
    }

    @Test
    fun `converting to device coordinates doubles the offset and flips y`() {
        // The frame spans 2 units in NDC, so a fraction-of-frame correction
        // doubles. And offsetY is raster-ordered (positive means the content is
        // lower in the picture) while NDC y points up, so it must be negated.
        // Applying it unflipped does not fail loudly: it drives the frame the
        // wrong way and roughly doubles the vertical wobble.
        val track = StabilizationTrack(
            keyframes = listOf(StabilizationKeyframe(0L, offsetX = 0.05f, offsetY = 0.04f)),
            zoom = 1.1f,
        )

        val (x, y) = track.ndcTranslationAt(0L)
        assertEquals(0.10f, x, 0.0001f)
        assertEquals(-0.08f, y, 0.0001f)
    }

    @Test
    fun `a clip gets the track rebased onto its own timeline`() {
        val rebased = fourSecondTrack.forClip(Clip(seconds(1.0), seconds(3.0)))

        assertEquals(
            listOf(seconds(0.0), seconds(1.0), seconds(2.0)),
            rebased.keyframes.map { it.timeUs },
        )
        assertEquals(0.1f, rebased.keyframes[0].offsetX, 0.001f)
        assertEquals(0.3f, rebased.keyframes[2].offsetX, 0.001f)
        assertEquals(1.05f, rebased.zoom, 0.001f)
    }

    @Test
    fun `rebasing a sped up clip compresses the keyframe times`() {
        val rebased = fourSecondTrack.forClip(Clip(seconds(1.0), seconds(3.0), speed = 2f))

        assertEquals(
            listOf(seconds(0.0), seconds(0.5), seconds(1.0)),
            rebased.keyframes.map { it.timeUs },
        )
    }

    @Test
    fun `a clip that starts between keyframes still begins at the right offset`() {
        val rebased = fourSecondTrack.forClip(Clip(seconds(1.5), seconds(2.5)))

        assertEquals(0L, rebased.keyframes.first().timeUs)
        assertEquals(0.15f, rebased.keyframes.first().offsetX, 0.001f)
        assertEquals(0.25f, rebased.keyframes.last().offsetX, 0.001f)
    }
}
