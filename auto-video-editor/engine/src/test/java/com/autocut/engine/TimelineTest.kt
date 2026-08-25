package com.autocut.engine

import com.autocut.engine.Fixtures.range
import com.autocut.engine.Fixtures.seconds
import com.autocut.engine.model.Clip
import com.autocut.engine.plan.SpeedRamp
import com.autocut.engine.plan.Timeline
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TimelineTest {

    @Test
    fun `no cuts leaves one clip covering the source`() {
        val clips = Timeline.assemble(seconds(10.0), emptyList())
        assertEquals(listOf(Clip(0L, seconds(10.0))), clips)
    }

    @Test
    fun `a cut in the middle produces two clips`() {
        val clips = Timeline.assemble(seconds(10.0), listOf(range(4.0, 6.0)))
        assertEquals(
            listOf(Clip(0L, seconds(4.0)), Clip(seconds(6.0), seconds(10.0))),
            clips,
        )
    }

    @Test
    fun `overlapping cuts are unioned rather than double counted`() {
        val clips = Timeline.assemble(
            seconds(10.0),
            listOf(range(3.0, 5.0), range(4.0, 6.0)),
        )
        assertEquals(
            listOf(Clip(0L, seconds(3.0)), Clip(seconds(6.0), seconds(10.0))),
            clips,
        )
    }

    @Test
    fun `clips too short to encode are absorbed into the cut`() {
        // The 50ms island between these two cuts is not an edit, it is a glitch.
        val clips = Timeline.assemble(
            seconds(10.0),
            listOf(range(4.0, 5.0), range(5.05, 6.0)),
            minClipUs = 120_000L,
        )
        assertEquals(
            listOf(Clip(0L, seconds(4.0)), Clip(seconds(6.0), seconds(10.0))),
            clips,
        )
    }

    @Test
    fun `a ramp splits its clip into normal and sped up pieces`() {
        val clips = Timeline.assemble(
            durationUs = seconds(10.0),
            cuts = emptyList(),
            ramps = listOf(SpeedRamp(range(4.0, 6.0), 2f)),
        )
        assertEquals(3, clips.size)
        assertEquals(Clip(0L, seconds(4.0), 1f), clips[0])
        assertEquals(Clip(seconds(4.0), seconds(6.0), 2f), clips[1])
        assertEquals(Clip(seconds(6.0), seconds(10.0), 1f), clips[2])
    }

    @Test
    fun `a ramp shortens the output by its speed`() {
        val clips = Timeline.assemble(
            durationUs = seconds(10.0),
            cuts = emptyList(),
            ramps = listOf(SpeedRamp(range(0.0, 8.0), 4f)),
        )
        // 8 seconds at 4x is 2, plus the 2 untouched seconds.
        assertEquals(seconds(4.0), clips.sumOf { it.outputDurationUs })
    }

    @Test
    fun `overlapping ramps do not double apply`() {
        val clips = Timeline.assemble(
            durationUs = seconds(10.0),
            cuts = emptyList(),
            ramps = listOf(SpeedRamp(range(2.0, 6.0), 2f), SpeedRamp(range(4.0, 8.0), 3f)),
        )
        assertTrue(clips.all { it.speed == 1f || it.speed == 2f })
        assertEquals(seconds(10.0), clips.sumOf { it.sourceDurationUs })
    }

    @Test
    fun `cutting everything returns nothing rather than a zero length clip`() {
        assertTrue(Timeline.assemble(seconds(10.0), listOf(range(0.0, 10.0))).isEmpty())
    }

    @Test
    fun `clips never overlap and stay in source order`() {
        val clips = Timeline.assemble(
            durationUs = seconds(60.0),
            cuts = listOf(range(5.0, 7.0), range(20.0, 25.0), range(40.0, 41.0)),
            ramps = listOf(SpeedRamp(range(30.0, 35.0), 2f)),
        )
        for (i in 1 until clips.size) {
            assertTrue(clips[i].sourceStartUs >= clips[i - 1].sourceEndUs)
        }
    }
}
