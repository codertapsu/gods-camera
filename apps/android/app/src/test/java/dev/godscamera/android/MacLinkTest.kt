package dev.godscamera.android

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.test.TestScope
import kotlinx.coroutines.test.advanceTimeBy
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runCurrent
import kotlinx.coroutines.test.runTest
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Keeping the connection, with the network played by a table of who answers where. */
@OptIn(ExperimentalCoroutinesApi::class)
class MacLinkTest {
    private val fingerprint = fingerprintOf("the Mac's certificate".toByteArray())
    private val mac = PairedMac("MacBook", fingerprint, "192.168.1.20:47801", "device-1")
    private val announced = FoundMac("MacBook", listOf("192.168.1.31:47801", "[fd00::31]:47801"), fingerprint.take(16))

    private class World(scope: TestScope, paired: PairedMac?) {
        val preferences = FakePreferences()
        val store = PairingStore(preferences, FakeBox()).also { if (paired != null) it.save(paired, "token") }
        val found = MutableStateFlow<List<FoundMac>>(emptyList())
        /** Who answers at each address. Anything not listed is silent. */
        val answers = mutableMapOf<String, Answer>()
        val asked = mutableListOf<String>()
        val link = MacLink(scope.backgroundScope, store, found, ask = { address, _, _ ->
            asked += address
            answers[address] ?: Answer.Silent
        })
    }

    @Test
    fun withoutAMacThereIsNothingToLookFor() = runTest {
        val world = World(this, paired = null)
        world.link.begin()
        advanceUntilIdle()
        assertEquals(Connection.NotPaired, world.link.connection.value)
        assertEquals(emptyList<String>(), world.asked)
        assertNull(world.link.client())
    }

    @Test
    fun theMacIsFoundWhereItWasLastTime() = runTest {
        val world = World(this, mac)
        world.answers[mac.address] = Answer.Accepted
        world.link.begin()
        assertEquals(Connection.Searching("MacBook"), world.link.connection.value)
        runCurrent()
        assertEquals(Connection.Connected("MacBook"), world.link.connection.value)
        assertEquals(listOf(mac.address), world.asked)
    }

    @Test
    fun aMacThatMovedIsFoundByItsAnnouncementAndRemembered() = runTest {
        val world = World(this, mac)
        world.answers["[fd00::31]:47801"] = Answer.Accepted
        world.link.begin()
        runCurrent()
        // The old address is silent, and nothing has been announced yet.
        assertEquals(Connection.Searching("MacBook"), world.link.connection.value)
        // Somebody else's Mac comes into sight first; it is not this device's business.
        world.found.value = listOf(FoundMac("Other Mac", listOf("192.168.1.99:47801"), "0000000000000000"))
        advanceTimeBy(900)
        assertEquals(listOf(mac.address), world.asked)
        world.found.value = world.found.value + announced
        advanceTimeBy(900)
        assertEquals(Connection.Connected("MacBook"), world.link.connection.value)
        // Each place was tried once, in the announced order.
        assertEquals(listOf(mac.address, "192.168.1.31:47801", "[fd00::31]:47801"), world.asked)
        assertEquals("[fd00::31]:47801", world.store.load()?.first?.address)
        assertEquals("[fd00::31]:47801", world.link.mac?.address)
    }

    @Test
    fun aMacThatDoesNotAnswerIsGivenUpOnAndTriedAgainWhenItAppears() = runTest {
        val world = World(this, mac)
        world.link.begin()
        advanceTimeBy(7000)
        assertEquals(Connection.Searching("MacBook"), world.link.connection.value)
        advanceTimeBy(2000)
        assertEquals(Connection.NotFound("MacBook"), world.link.connection.value)
        // The Mac is opened later and announces itself.
        world.answers["192.168.1.31:47801"] = Answer.Accepted
        world.found.value = listOf(announced)
        advanceTimeBy(500)
        assertEquals(Connection.Connected("MacBook"), world.link.connection.value)
    }

    @Test
    fun aMacThatRemovedThisDeviceSaysSoAtOnce() = runTest {
        val world = World(this, mac)
        world.answers[mac.address] = Answer.Refused
        world.link.begin()
        runCurrent()
        assertEquals(Connection.Refused("MacBook"), world.link.connection.value)
        // It stays that way: looking harder does not help, a new code does.
        advanceTimeBy(20_000)
        assertEquals(Connection.Refused("MacBook"), world.link.connection.value)
        assertEquals(listOf(mac.address), world.asked)
    }

    @Test
    fun connectingStoresTheMacAndForgettingRemovesEveryTraceOfIt() = runTest {
        val world = World(this, paired = null)
        world.link.begin()
        world.answers[mac.address] = Answer.Accepted
        world.link.pairedWith(mac, "the-secret-token")
        runCurrent()
        assertEquals(Connection.Connected("MacBook"), world.link.connection.value)
        assertEquals(mac to "the-secret-token", world.store.load())

        world.link.forget()
        assertEquals(Connection.NotPaired, world.link.connection.value)
        assertNull(world.store.load())
        assertNull(world.link.mac)
        assertEquals(emptyMap<String, Any?>(), world.preferences.values)
    }

    @Test
    fun aConnectionThatBreaksIsNoticedOnTheNextLook() = runTest {
        val world = World(this, mac)
        world.answers[mac.address] = Answer.Accepted
        world.link.begin()
        runCurrent()
        assertEquals(Connection.Connected("MacBook"), world.link.connection.value)
        // The Mac goes to sleep. The screen keeps saying "connected" until the look has failed.
        world.answers.clear()
        world.link.recheck()
        runCurrent()
        assertEquals(Connection.Connected("MacBook"), world.link.connection.value)
        advanceTimeBy(9000)
        assertEquals(Connection.NotFound("MacBook"), world.link.connection.value)
    }
}
