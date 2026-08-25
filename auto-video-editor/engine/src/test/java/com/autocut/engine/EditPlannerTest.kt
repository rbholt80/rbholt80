package com.autocut.engine

import com.autocut.engine.Fixtures.range
import com.autocut.engine.Fixtures.seconds
import com.autocut.engine.analysis.MediaAnalyzer
import com.autocut.engine.model.EditPreferences
import com.autocut.engine.model.EditStyle
import com.autocut.engine.model.MediaSignals
import com.autocut.engine.plan.EditPlanner
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class EditPlannerTest {

    private fun plan(signals: MediaSignals, prefs: EditPreferences = EditPreferences()) =
        EditPlanner.plan(MediaAnalyzer.analyze(signals), prefs)

    /** 30s of speech with dead air at both ends and one 3s pause in the middle. */
    private fun talkingHead(): MediaSignals {
        val duration = seconds(30.0)
        return Fixtures.signals(
            durationUs = duration,
            audio = Fixtures.speechWithSilences(
                duration,
                listOf(range(0.0, 2.0), range(10.0, 13.0), range(28.0, 30.0)),
            ),
        )
    }

    // ---- the automatic path ------------------------------------------------

    @Test
    fun `a talking head gets its edges trimmed and its pause cut`() {
        val plan = plan(talkingHead())

        assertEquals(
            listOf(
                EditPlanner.ID_TRIM_EDGES,
                EditPlanner.ID_REMOVE_SILENCE,
                EditPlanner.ID_NORMALIZE_LOUDNESS,
            ),
            plan.fixes.map { it.id },
        )
        assertTrue(plan.fixes.all { it.enabled })

        // 1.7s off each end plus a 2.6s pause, leaving a 0.18s breath at the cut.
        assertEquals(seconds(6.0), plan.removedDurationUs)
        assertEquals(seconds(24.0), plan.outputDurationUs)
        assertEquals(2, plan.clips.size)
        assertEquals(seconds(1.7), plan.clips[0].sourceStartUs)
        assertEquals(seconds(10.2), plan.clips[0].sourceEndUs)
        assertEquals(seconds(12.8), plan.clips[1].sourceStartUs)
        assertEquals(seconds(28.3), plan.clips[1].sourceEndUs)
    }

    @Test
    fun `a clean picture is left alone`() {
        val plan = plan(talkingHead())
        assertTrue("nothing was wrong with the picture", plan.video.isIdentity)
    }

    @Test
    fun `the level is brought to the target`() {
        val plan = plan(talkingHead())
        // Recorded at -20 dBFS, target -16, peaks at -8 so there is room without a limiter.
        assertEquals(4f, plan.audio.gainDb, 0.2f)
        assertFalse(plan.audio.limiterEnabled)
        assertNull(plan.fix(EditPlanner.ID_LIMIT_PEAKS))
    }

    @Test
    fun `planning twice gives exactly the same plan`() {
        val analysis = MediaAnalyzer.analyze(talkingHead())
        assertEquals(EditPlanner.plan(analysis), EditPlanner.plan(analysis))
    }

    @Test
    fun `every fix explains itself with a measurement`() {
        val plan = plan(talkingHead())
        for (fix in plan.fixes) {
            assertTrue("${fix.id} has no detail", fix.detail.isNotBlank())
            assertTrue("${fix.id} states no number", fix.detail.any { it.isDigit() })
        }
    }

    // ---- overruling the automatic decision ---------------------------------

    @Test
    fun `switching off a cut restores that time without disturbing the others`() {
        val analysis = MediaAnalyzer.analyze(talkingHead())
        val auto = EditPlanner.plan(analysis)
        val manual = EditPlanner.plan(
            analysis,
            EditPreferences().withFix(EditPlanner.ID_REMOVE_SILENCE, false),
        )

        assertEquals(seconds(24.0), auto.outputDurationUs)
        assertEquals(seconds(26.6), manual.outputDurationUs)

        val fix = manual.fix(EditPlanner.ID_REMOVE_SILENCE)
        assertNotNull(fix)
        assertFalse(fix!!.enabled)
        // It still reports what it would have saved, so the offer stays meaningful.
        assertEquals(seconds(2.6), fix.savedUs)

        // The unrelated decisions are untouched.
        assertEquals(auto.audio, manual.audio)
        assertEquals(auto.fix(EditPlanner.ID_TRIM_EDGES), manual.fix(EditPlanner.ID_TRIM_EDGES))
    }

    @Test
    fun `switching everything off returns the untouched source`() {
        val analysis = MediaAnalyzer.analyze(talkingHead())
        var prefs = EditPreferences()
        for (fix in EditPlanner.plan(analysis).fixes) {
            prefs = prefs.withFix(fix.id, false)
        }
        val plan = EditPlanner.plan(analysis, prefs)

        assertEquals(1, plan.clips.size)
        assertEquals(seconds(30.0), plan.outputDurationUs)
        assertTrue(plan.audio.isIdentity)
        assertTrue(plan.video.isIdentity)
        assertFalse(plan.changesAnything)
    }

    @Test
    fun `resetting the overrides gives the automatic plan back`() {
        val analysis = MediaAnalyzer.analyze(talkingHead())
        val prefs = EditPreferences().withFix(EditPlanner.ID_REMOVE_SILENCE, false)
        assertEquals(EditPlanner.plan(analysis), EditPlanner.plan(analysis, prefs.reset()))
    }

    // ---- styles ------------------------------------------------------------

    @Test
    fun `cleanup only fixes the sound without touching the timeline`() {
        val plan = plan(talkingHead(), EditPreferences(style = EditStyle.CLEANUP_ONLY))

        assertEquals(1, plan.clips.size)
        assertEquals(seconds(30.0), plan.outputDurationUs)
        assertTrue(plan.fixes.none { it.cuts })
        assertNotNull(plan.fix(EditPlanner.ID_NORMALIZE_LOUDNESS))
    }

    @Test
    fun `tighter styles cut more than gentler ones`() {
        val analysis = MediaAnalyzer.analyze(talkingHead())
        val lengths = listOf(
            EditStyle.CLEANUP_ONLY,
            EditStyle.LIGHT_TOUCH,
            EditStyle.BALANCED,
            EditStyle.TIGHT,
        ).map { EditPlanner.plan(analysis, EditPreferences(style = it)).outputDurationUs }

        for (i in 1 until lengths.size) {
            assertTrue(
                "styles should be ordered by how much they cut, got $lengths",
                lengths[i] <= lengths[i - 1],
            )
        }
    }

    // ---- guards ------------------------------------------------------------

    @Test
    fun `the removal budget stops a mostly silent clip being cut to nothing`() {
        val duration = seconds(40.0)
        val silences = listOf(
            range(1.0, 7.0), range(8.0, 14.0), range(15.0, 21.0),
            range(22.0, 28.0), range(29.0, 35.0), range(36.0, 40.0),
        )
        val plan = plan(
            Fixtures.signals(duration, audio = Fixtures.speechWithSilences(duration, silences))
        )

        // 60% of 40s is the ceiling for the Balanced style.
        assertTrue("removed ${plan.removedDurationUs}", plan.removedDurationUs <= seconds(24.0))
        assertTrue(plan.outputDurationUs >= seconds(3.2))

        // Longest pauses first, so the budget buys the biggest wins: three of the
        // five 5.6s cuts fit alongside the 3.7s tail trim.
        assertEquals(seconds(16.8), plan.fix(EditPlanner.ID_REMOVE_SILENCE)!!.savedUs)
        assertEquals(seconds(19.5), plan.outputDurationUs)
    }

    @Test
    fun `a clip with no sound is not cut and its levels are not touched`() {
        val duration = seconds(30.0)
        val plan = plan(
            Fixtures.signals(
                duration,
                audio = emptyList(),
                probe = Fixtures.probe(duration, hasAudio = false),
            )
        )

        assertEquals(1, plan.clips.size)
        assertTrue(plan.audio.isIdentity)
        assertTrue(plan.notes.any { it.id == "no_audio" })
    }

    @Test
    fun `audio with no quiet parts is left uncut and the user is told why`() {
        val duration = seconds(30.0)
        val plan = plan(Fixtures.signals(duration, audio = Fixtures.flatAudio(duration)))

        assertEquals(1, plan.clips.size)
        assertTrue(plan.fixes.none { it.cuts })
        assertTrue(plan.notes.any { it.id == "audio_flat" })
    }

    @Test
    fun `a zero length source produces a plan instead of throwing`() {
        val plan = plan(Fixtures.signals(0L, audio = emptyList(), video = emptyList()))
        assertTrue(plan.fixes.isEmpty())
        assertTrue(plan.notes.any { it.id == "unreadable" })
    }

    @Test
    fun `clips always stay inside the source and in order`() {
        val plan = plan(talkingHead())
        var previousEnd = 0L
        for (clip in plan.clips) {
            assertTrue(clip.sourceStartUs >= previousEnd)
            assertTrue(clip.sourceEndUs <= plan.source.durationUs)
            assertTrue(clip.sourceDurationUs > 0)
            previousEnd = clip.sourceEndUs
        }
    }
}
