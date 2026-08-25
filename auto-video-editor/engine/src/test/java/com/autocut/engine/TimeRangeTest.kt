package com.autocut.engine

import com.autocut.engine.model.TimeRange
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class TimeRangeTest {

    @Test
    fun `merge unions overlapping and touching ranges`() {
        val merged = TimeRange.merge(
            listOf(
                TimeRange(0, 100),
                TimeRange(50, 120),   // overlaps the first
                TimeRange(120, 200),  // touches the second
                TimeRange(400, 500),
            )
        )
        assertEquals(listOf(TimeRange(0, 200), TimeRange(400, 500)), merged)
    }

    @Test
    fun `merge drops empty ranges and sorts unordered input`() {
        val merged = TimeRange.merge(
            listOf(TimeRange(400, 500), TimeRange(70, 70), TimeRange(0, 100))
        )
        assertEquals(listOf(TimeRange(0, 100), TimeRange(400, 500)), merged)
    }

    @Test
    fun `merge of a fully contained range keeps the outer bounds`() {
        assertEquals(
            listOf(TimeRange(0, 1000)),
            TimeRange.merge(listOf(TimeRange(0, 1000), TimeRange(200, 300)))
        )
    }

    @Test
    fun `complement returns the gaps between ranges`() {
        val gaps = TimeRange.complement(
            TimeRange(0, 1000),
            listOf(TimeRange(100, 200), TimeRange(500, 600))
        )
        assertEquals(
            listOf(TimeRange(0, 100), TimeRange(200, 500), TimeRange(600, 1000)),
            gaps
        )
    }

    @Test
    fun `complement of the whole span is empty`() {
        assertTrue(TimeRange.complement(TimeRange(0, 1000), listOf(TimeRange(0, 1000))).isEmpty())
    }

    @Test
    fun `complement clamps ranges that overhang the bounds`() {
        val gaps = TimeRange.complement(
            TimeRange(100, 900),
            listOf(TimeRange(-500, 200), TimeRange(800, 5000))
        )
        assertEquals(listOf(TimeRange(200, 800)), gaps)
    }

    @Test
    fun `contains is half open`() {
        val range = TimeRange(100, 200)
        assertTrue(100L in range)
        assertTrue(199L in range)
        assertTrue(200L !in range)
    }
}
