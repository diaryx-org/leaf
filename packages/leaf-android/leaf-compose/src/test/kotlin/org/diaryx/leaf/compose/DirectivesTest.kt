package org.diaryx.leaf.compose

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class DirectivesTest {
    private fun embed(src: String) = DirectiveKey("embed", "", listOf("src" to src))

    @Test
    fun identicalDirectivesTakeTwoSlots() {
        val a = embed("https://a.example")
        val b = embed("https://b.example")
        assertEquals(listOf(a to 0, b to 0, a to 1), slotKeys(listOf(a, b, a)))
    }

    @Test
    fun aSlotSurvivesAnEditAboveADifferentDirective() {
        val a = embed("https://a.example")
        val b = embed("https://b.example")
        // A directive inserted above `a` does not renumber it.
        assertEquals(slotKeys(listOf(a))[0], slotKeys(listOf(b, a))[1])
    }

    @Test
    fun theKeyIsEverythingTheDirectiveSays() {
        assertTrue(embed("x") == embed("x"))
        assertFalse(embed("x") == DirectiveKey("embed", "Label", listOf("src" to "x")))
        assertFalse(embed("x") == DirectiveKey("video", "", listOf("src" to "x")))
    }

    @Test
    fun aPointLandsOnTheNearerSide() {
        // A drawing from x=100 to x=300; the stops are ch 2 (past a `│ ` gutter) and ch 9.
        assertEquals(2, directiveStop(x = 120f, left = 100f, width = 200f, before = 2, after = 9))
        assertEquals(2, directiveStop(x = 50f, left = 100f, width = 200f, before = 2, after = 9))
        assertEquals(9, directiveStop(x = 200f, left = 100f, width = 200f, before = 2, after = 9))
        assertEquals(9, directiveStop(x = 900f, left = 100f, width = 200f, before = 2, after = 9))
    }

    @Test
    fun onlyTheRowsEndIsAfter() {
        // `⧉ embed`: every glyph is the directive's start, the row's end is past it.
        assertFalse(isAfterDirective(0, 7))
        assertFalse(isAfterDirective(3, 7))
        assertTrue(isAfterDirective(7, 7))
    }
}
