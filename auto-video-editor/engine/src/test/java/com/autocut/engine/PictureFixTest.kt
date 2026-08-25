package com.autocut.engine

import com.autocut.engine.Fixtures.seconds
import com.autocut.engine.analysis.MediaAnalyzer
import com.autocut.engine.model.EditPlan
import com.autocut.engine.model.MediaSignals
import com.autocut.engine.plan.EditPlanner
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class PictureFixTest {

    private fun planFor(video: List<com.autocut.engine.model.VideoSample>, probe: com.autocut.engine.model.MediaProbe? = null): EditPlan {
        val duration = seconds(30.0)
        val signals = MediaSignals(
            probe = probe ?: Fixtures.probe(duration, hasAudio = false),
            audio = emptyList(),
            video = video,
        )
        return EditPlanner.plan(MediaAnalyzer.analyze(signals))
    }

    @Test
    fun `an underexposed shot is brightened`() {
        val plan = planFor(Fixtures.steadyVideo(seconds(30.0), meanLuma = 60f))

        assertNotNull(plan.fix(EditPlanner.ID_FIX_EXPOSURE))
        // 118/60 is 1.97, capped at the 1.75x the engine is willing to push.
        assertEquals(1.75f, plan.video.redScale, 0.01f)
        assertEquals(plan.video.redScale, plan.video.blueScale, 0.001f)
    }

    @Test
    fun `a blown out shot is not brightened further`() {
        // Dim average, but a third of the frame is already pure white — the
        // average is low because of a dark background, not underexposure.
        val plan = planFor(
            Fixtures.steadyVideo(seconds(30.0), meanLuma = 70f)
                .map { it.copy(highlightRatio = 0.3f) }
        )

        assertEquals(1.08f, plan.video.redScale, 0.01f)
    }

    @Test
    fun `a correctly exposed shot gets no exposure fix`() {
        val plan = planFor(Fixtures.steadyVideo(seconds(30.0), meanLuma = 118f))
        assertNull(plan.fix(EditPlanner.ID_FIX_EXPOSURE))
    }

    @Test
    fun `a warm cast is cooled by pulling red down and blue up`() {
        val plan = planFor(
            Fixtures.steadyVideo(seconds(30.0))
                .map { it.copy(meanU = 118f, meanV = 148f) }
        )

        assertNotNull(plan.fix(EditPlanner.ID_FIX_WHITE_BALANCE))
        assertTrue("red should come down, was ${plan.video.redScale}", plan.video.redScale < 0.98f)
        assertTrue("blue should go up, was ${plan.video.blueScale}", plan.video.blueScale > 1.02f)
    }

    @Test
    fun `a strong cast is only partly corrected and flagged as possibly deliberate`() {
        val plan = planFor(
            Fixtures.steadyVideo(seconds(30.0))
                .map { it.copy(meanU = 80f, meanV = 180f) }
        )

        assertTrue(plan.notes.any { it.id == "strong_cast" })
        // Half-strength damping, so the correction stays well short of neutral.
        assertTrue(plan.video.redScale > 0.86f)
    }

    @Test
    fun `a flat picture gets contrast`() {
        val plan = planFor(Fixtures.steadyVideo(seconds(30.0), lumaStdDev = 20f))

        assertNotNull(plan.fix(EditPlanner.ID_FIX_CONTRAST))
        assertEquals(0.30f, plan.video.contrast, 0.01f)
    }

    @Test
    fun `washed out colour is brought back`() {
        val plan = planFor(Fixtures.steadyVideo(seconds(30.0), meanChroma = 0.05f))

        val fix = plan.fix(EditPlanner.ID_BOOST_SATURATION)
        assertNotNull(fix)
        assertTrue(fix!!.enabled)
        assertEquals(30f, plan.video.saturationPercent, 0.5f)
    }

    @Test
    fun `heavy colour is offered as a fix but left switched off`() {
        val plan = planFor(Fixtures.steadyVideo(seconds(30.0), meanChroma = 0.45f))

        val fix = plan.fix(EditPlanner.ID_BOOST_SATURATION)
        assertNotNull(fix)
        assertFalse("strong colour is usually a look, not a fault", fix!!.enabled)
        assertEquals(0f, plan.video.saturationPercent, 0.001f)
    }

    @Test
    fun `out of focus takes are cut even with no soundtrack`() {
        val soft = Fixtures.steadyVideo(seconds(30.0)).map {
            if (it.timeUs >= seconds(12.0) && it.timeUs < seconds(14.0)) {
                it.copy(sharpness = 20f, motion = 0.005f)
            } else {
                it
            }
        }
        val plan = planFor(soft)

        val fix = plan.fix(EditPlanner.ID_REMOVE_SOFT_FOCUS)
        assertNotNull(fix)
        assertTrue(fix!!.enabled)
        assertEquals(seconds(2.0), plan.removedDurationUs)
        assertEquals(2, plan.clips.size)
    }

    @Test
    fun `a shot that is soft throughout is not cut to pieces`() {
        val soft = Fixtures.steadyVideo(seconds(30.0)).map {
            if (it.timeUs >= seconds(9.0) && it.timeUs < seconds(21.0)) {
                it.copy(sharpness = 20f, motion = 0.005f)
            } else {
                it
            }
        }
        val plan = planFor(soft)

        val fix = plan.fix(EditPlanner.ID_REMOVE_SOFT_FOCUS)
        assertNotNull(fix)
        assertFalse(fix!!.enabled)
        assertEquals(1, plan.clips.size)
        assertTrue(plan.notes.any { it.id == "mostly_soft" })
    }

    @Test
    fun `oversized video is offered a downscale, ordinary video is not`() {
        val duration = seconds(30.0)
        val fourK = planFor(
            Fixtures.steadyVideo(duration),
            probe = Fixtures.probe(duration, width = 3840, height = 2160, hasAudio = false),
        )
        val fix = fourK.fix(EditPlanner.ID_DOWNSCALE)
        assertNotNull(fix)
        assertTrue(fix!!.enabled)
        assertEquals(1080, fourK.video.maxShortSidePx)

        val fullHd = planFor(
            Fixtures.steadyVideo(duration),
            probe = Fixtures.probe(duration, width = 1920, height = 1080, hasAudio = false),
        )
        assertNull(fullHd.fix(EditPlanner.ID_DOWNSCALE))
    }

    @Test
    fun `a portrait clip is measured by its short edge, not its height`() {
        val duration = seconds(30.0)
        val plan = planFor(
            Fixtures.steadyVideo(duration),
            probe = Fixtures.probe(duration, width = 1080, height = 1920, hasAudio = false),
        )
        // 1080x1920 is a 1080p phone video, not something that needs shrinking.
        assertNull(plan.fix(EditPlanner.ID_DOWNSCALE))
    }
}
