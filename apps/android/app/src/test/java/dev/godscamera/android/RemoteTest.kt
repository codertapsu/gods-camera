package dev.godscamera.android

import kotlinx.serialization.json.Json
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** The Remote tab's side of api/remote.md. */
class RemoteTest {
    private val json = Json { ignoreUnknownKeys = true }

    @Test
    fun theStateIsReadAsTheHubWritesIt() {
        val state = json.decodeFromString<RemoteState>(
            """{"mode":"looping","recording":false,"recorded_seconds":0.0,"can_record":false,"can_loop":true,
               "swap_wanted":true,"swapping":false,"swap_problem":"no_face_seen","selected_face":"a","watched":true,
               "something_newer":42}""",
        )
        assertTrue(state.looping && state.can_loop && state.swap_wanted && state.watched)
        assertEquals("no_face_seen", state.swap_problem)
        assertEquals("a", state.selected_face)
        // A hub that leaves fields out gives the defaults.
        val bare = json.decodeFromString<RemoteState>("""{"mode":"live"}""")
        assertFalse(bare.looping || bare.swap_wanted)
        assertNull(bare.swap_problem)
        val faces = json.decodeFromString<List<RemoteFace>>("""[{"id":"a","name":"Mặt mẫu"}]""")
        assertEquals(listOf(RemoteFace("a", "Mặt mẫu")), faces)
    }

    @Test
    fun everyCodeTheComputerSendsHasWords() {
        for (code in remoteCodes) assertNotNull(code, remoteMessage(code))
        assertNull(remoteMessage("something_new"))
    }

    @Test
    fun theCodeIsTakenFromTheHubsAnswer() {
        assertEquals("no_loop_yet", HubException(409, """{"error":"no_loop_yet","request_id":"x"}""").code)
        assertNull(HubException(500, "not json at all").code)
        assertNull(HubException(500, """{"request_id":"x"}""").code)
    }
}
