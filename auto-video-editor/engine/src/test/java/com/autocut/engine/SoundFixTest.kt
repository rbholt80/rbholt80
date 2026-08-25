package com.autocut.engine

import com.autocut.engine.Fixtures.range
import com.autocut.engine.Fixtures.seconds
import com.autocut.engine.analysis.MediaAnalyzer
import com.autocut.engine.model.EditPreferences
import com.autocut.engine.plan.EditPlanner
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertTrue
import org.junit.Test

class SoundFixTest {

    /** Recorded far too quietly, but with peaks already almost at full scale. */
    private fun quietWithHotPeaks(): com.autocut.engine.model.MediaSignals {
        val duration = seconds(20.0)
        return Fixtures.signals(
            duration,
            audio = Fixtures.speechWithSilences(
                duration,
                listOf(range(0.0, 3.0)),
                speechRms = 0.02f,   // -34 dBFS
                speechPeak = 0.9f,   // -0.9 dBFS
            ),
        )
    }

    @Test
    fun `a quiet recording with hot peaks gets a limiter as well as gain`() {
        val plan = EditPlanner.plan(MediaAnalyzer.analyze(quietWithHotPeaks()))

        assertNotNull(plan.fix(EditPlanner.ID_NORMALIZE_LOUDNESS))
        assertNotNull(plan.fix(EditPlanner.ID_LIMIT_PEAKS))
        assertTrue(plan.audio.limiterEnabled)
        // 18 dB is needed to reach the target but there is no headroom, so the
        // gain is capped at what the limiter can honestly hold back.
        assertEquals(5.9f, plan.audio.gainDb, 0.2f)
    }

    @Test
    fun `turning the limiter off pulls the gain back to the real headroom`() {
        val analysis = MediaAnalyzer.analyze(quietWithHotPeaks())
        val plan = EditPlanner.plan(
            analysis,
            EditPreferences().withFix(EditPlanner.ID_LIMIT_PEAKS, false),
        )

        assertFalse(plan.audio.limiterEnabled)
        // Keeping +5.9 dB without a limiter would drive the peaks into clipping,
        // so the gain has to come down with it.
        assertEquals(0f, plan.audio.gainDb, 0.2f)
        assertTrue(plan.audio.gainDb < 1f)
    }

    @Test
    fun `already clipped audio is not pushed louder and the user is told`() {
        val duration = seconds(10.0)
        val samples = Fixtures.speechWithSilences(duration, listOf(range(0.0, 1.5)))
            .mapIndexed { index, sample ->
                if (index % 10 == 0 && sample.rms > Fixtures.ROOM_RMS) sample.copy(peak = 1f) else sample
            }
        val plan = EditPlanner.plan(MediaAnalyzer.analyze(Fixtures.signals(duration, audio = samples)))

        assertTrue(plan.notes.any { it.id == "audio_clipped" })
        assertTrue("clipped audio must never be boosted", plan.audio.gainDb <= 0f)
    }

    @Test
    fun `a level that is already right is left alone`() {
        val duration = seconds(20.0)
        val plan = EditPlanner.plan(
            MediaAnalyzer.analyze(
                Fixtures.signals(
                    duration,
                    audio = Fixtures.speechWithSilences(
                        duration,
                        listOf(range(0.0, 2.0)),
                        speechRms = 0.158f,  // -16 dBFS, exactly on target
                        speechPeak = 0.5f,
                    ),
                )
            )
        )

        assertEquals(null, plan.fix(EditPlanner.ID_NORMALIZE_LOUDNESS))
        assertTrue(plan.audio.isIdentity)
    }
}
