package com.autocut.engine

import com.autocut.engine.Fixtures.range
import com.autocut.engine.Fixtures.seconds
import com.autocut.engine.analysis.AudioAnalyzer
import com.autocut.engine.analysis.Dsp
import com.autocut.engine.model.AudioSample
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test
import kotlin.math.PI
import kotlin.math.sin

class AudioAnalyzerTest {

    @Test
    fun `finds the quiet runs in speech`() {
        val duration = seconds(30.0)
        val samples = Fixtures.speechWithSilences(
            duration,
            listOf(range(0.0, 2.0), range(10.0, 13.0), range(28.0, 30.0)),
        )

        val profile = AudioAnalyzer.analyze(samples, duration)

        assertTrue(profile.reliable)
        assertEquals(3, profile.silences.size)
        assertEquals(seconds(0.0), profile.silences[0].range.startUs)
        assertEquals(seconds(2.0), profile.silences[0].range.endUs)
        assertEquals(seconds(10.0), profile.silences[1].range.startUs)
        assertEquals(seconds(13.0), profile.silences[1].range.endUs)
        assertEquals(seconds(30.0), profile.silences[2].range.endUs)
    }

    @Test
    fun `threshold sits between the room tone and the speech`() {
        val duration = seconds(30.0)
        val samples = Fixtures.speechWithSilences(duration, listOf(range(10.0, 13.0)))

        val profile = AudioAnalyzer.analyze(samples, duration)

        assertEquals(-60f, profile.noiseFloorDb, 1f)
        assertEquals(-20f, profile.loudDb, 1f)
        assertTrue(
            "threshold ${profile.thresholdDb} must sit between room tone and speech",
            profile.thresholdDb > profile.noiseFloorDb && profile.thresholdDb < profile.loudDb,
        )
    }

    @Test
    fun `a well paced take with only a little silence is still usable`() {
        val duration = Fixtures.seconds(30.0)
        // One 3s pause in 30s. At a tenth of the running time the pauses no
        // longer reach the 10th percentile, which is where a purely
        // percentile-based noise floor quietly stops working.
        val profile = AudioAnalyzer.analyze(
            Fixtures.speechWithSilences(duration, listOf(range(10.0, 13.0))),
            duration,
        )

        assertTrue(profile.reliable)
        assertEquals(1, profile.silences.size)
        assertEquals(-60f, profile.noiseFloorDb, 1f)
    }

    @Test
    fun `even a single short pause in a long take is found`() {
        val duration = seconds(60.0)
        // 1.7% of the clip. The threshold has to come from the distance below
        // the speech, because no low percentile will land in a pause this rare.
        val profile = AudioAnalyzer.analyze(
            Fixtures.speechWithSilences(duration, listOf(range(30.0, 31.0))),
            duration,
        )

        assertTrue(profile.reliable)
        assertEquals(1, profile.silences.size)
        assertEquals(seconds(30.0), profile.silences.first().range.startUs)
    }

    @Test
    fun `program level measures the speech and not the silence`() {
        val duration = seconds(30.0)
        // Two thirds of this clip is silence; averaging over all of it would
        // report a level roughly 5 dB too low and over-boost the result.
        val samples = Fixtures.speechWithSilences(duration, listOf(range(0.0, 20.0)))

        val profile = AudioAnalyzer.analyze(samples, duration)

        assertEquals(-20f, profile.programDb, 0.5f)
    }

    @Test
    fun `a mostly silent recording is the most cuttable one, not the least`() {
        // 90% room tone: a long recording with one short burst of speech. Reading
        // the "loud parts" off a high percentile made that percentile describe
        // room tone, which put the threshold below the noise floor and declared
        // the file to have no usable silence — refusing to cut precisely the
        // recording that needs it most.
        val duration = seconds(60.0)
        val profile = AudioAnalyzer.analyze(
            Fixtures.speechWithSilences(duration, listOf(range(2.0, 56.0))),
            duration,
        )

        assertTrue(profile.reliable)
        assertEquals(1, profile.silences.size)
        assertEquals(seconds(2.0), profile.silences.first().range.startUs)
        assertEquals(seconds(56.0), profile.silences.first().range.endUs)
        assertEquals(-60f, profile.noiseFloorDb, 1f)
    }

    @Test
    fun `speech that swings in level is not mistaken for silence`() {
        // Real speech moves about 24 dB across a sentence. Every other fixture
        // here holds it constant, which hides the failure this exists to catch:
        // with pauses under a tenth of the clip, deriving the threshold from a
        // low percentile put it INSIDE the speech, and a third of the actual
        // words were reported as silence for the planner to cut out.
        val duration = seconds(60.0)
        val samples = buildList {
            var startUs = 0L
            var index = 0
            while (startUs < duration) {
                val roomTone = startUs < seconds(4.0)
                val speechDb = -26f + 12f * sin(index * 2f * PI.toFloat() / 40f)
                val rms = if (roomTone) Fixtures.ROOM_RMS else Dsp.dbToAmplitude(speechDb)
                add(AudioSample(startUs, Fixtures.WINDOW_US, rms, rms * 3f))
                startUs += Fixtures.WINDOW_US
                index++
            }
        }

        val profile = AudioAnalyzer.analyze(samples, duration)

        assertTrue(profile.reliable)
        assertEquals("only the head is quiet", 1, profile.silences.size)
        assertEquals(0L, profile.silences.first().range.startUs)
        assertEquals(seconds(4.0), profile.silences.first().range.endUs)
    }

    @Test
    fun `audio with no quiet parts is reported as unreliable`() {
        val duration = seconds(30.0)
        val profile = AudioAnalyzer.analyze(Fixtures.flatAudio(duration), duration)

        assertFalse(profile.reliable)
        assertTrue(profile.silences.isEmpty())
    }

    @Test
    fun `a single dip below the threshold does not split one pause in two`() {
        val duration = seconds(20.0)
        val samples = Fixtures.speechWithSilences(duration, listOf(range(5.0, 9.0)))
            .map { sample ->
                // One window inside the pause creeps just above the threshold —
                // a breath, a chair creak. Hysteresis should ride over it.
                if (sample.startUs == seconds(7.0)) sample.copy(rms = 0.0025f) else sample
            }

        val profile = AudioAnalyzer.analyze(samples, duration)

        assertEquals(1, profile.silences.size)
        assertEquals(seconds(5.0), profile.silences.first().range.startUs)
        assertEquals(seconds(9.0), profile.silences.first().range.endUs)
    }

    @Test
    fun `clipping is measured from the peaks`() {
        val duration = seconds(10.0)
        val samples = Fixtures.speechWithSilences(duration, listOf(range(0.0, 1.0)))
            .mapIndexed { index, sample -> if (index % 10 == 0) sample.copy(peak = 1f) else sample }

        val profile = AudioAnalyzer.analyze(samples, duration)

        assertEquals(0.1f, profile.clippedFraction, 0.01f)
        assertEquals(0f, profile.truePeakDb, 0.01f)
    }

    @Test
    fun `empty input is handled without throwing`() {
        val profile = AudioAnalyzer.analyze(emptyList(), seconds(10.0))
        assertFalse(profile.reliable)
        assertTrue(profile.silences.isEmpty())
    }
}
