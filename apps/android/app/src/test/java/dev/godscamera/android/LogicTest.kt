package dev.godscamera.android

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertThrows
import org.junit.Assert.assertTrue
import org.junit.Test
import java.net.InetAddress
import java.security.cert.CertificateException
import java.security.cert.CertificateFactory
import java.security.cert.X509Certificate

class LogicTest {
    // --- The code field ---

    @Test
    fun theCodeFieldKeepsSixDigitsAndDropsEverythingElse() {
        assertEquals("482913", codeDigits("482 913"))
        assertEquals("482913", codeDigits("482-913"))
        assertEquals("482913", codeDigits("48 29 13 77"))
        assertEquals("12", codeDigits("a1b2"))
        assertEquals("", codeDigits(""))
        // Digits of other scripts are not the Mac's digits.
        assertEquals("", codeDigits("١٢٣"))
        assertTrue(isCompleteCode("000000"))
        assertFalse(isCompleteCode("12345"))
        assertFalse(isCompleteCode("1234567"))
        assertFalse(isCompleteCode("12345a"))
    }

    // --- Addresses a person types ---

    @Test
    fun aTypedAddressGetsThePortAndLosesWhatWasPastedAround() {
        assertEquals("192.168.1.20:47801", parseMacAddress("192.168.1.20"))
        assertEquals("192.168.1.20:47801", parseMacAddress("  192.168.1.20 "))
        assertEquals("192.168.1.20:47991", parseMacAddress("192.168.1.20:47991"))
        assertEquals("192.168.1.20:47801", parseMacAddress("https://192.168.1.20/"))
        assertEquals("marcus-macbook.local:47801", parseMacAddress("marcus-macbook.local"))
        assertEquals("[fd00::12]:47801", parseMacAddress("[fd00::12]"))
        assertEquals("[fd00::12]:5000", parseMacAddress("[fd00::12]:5000"))
    }

    @Test
    fun whatCannotBeAnAddressIsTurnedDown() {
        for (bad in listOf("", "   ", "192.168.1.20:", "192.168.1.20:0", "192.168.1.20:70000", "192.168.1.20:abc", "a b", "host/path",
            "user@host", "fd00::12", "[fd00::12", "[fd00::12]x", ".local", "host.", "[]")) {
            assertNull("\"$bad\" should not pass", parseMacAddress(bad))
        }
    }

    // --- Fingerprints and the one certificate that is trusted ---

    private fun certificate(pem: String): X509Certificate =
        CertificateFactory.getInstance("X.509").generateCertificate(pem.byteInputStream()) as X509Certificate

    @Test
    fun aFingerprintIsTheSha256InLowercaseHexadecimal() {
        // The same value the shared core's own test expects for the same bytes.
        assertEquals("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad", fingerprintOf("abc".toByteArray()))
        assertTrue(isFingerprint(fingerprintOf(ByteArray(0))))
        assertFalse(isFingerprint("ba7816"))
        assertFalse(isFingerprint("BA7816BF8F01CFEA414140DE5DAE2223B00361A396177A9CB410FF61F20015AD"))
        assertTrue(sameFingerprint("AB12", "ab12"))
        assertFalse(sameFingerprint("ab12", "ab13"))
        assertFalse(sameFingerprint("ab12", "ab1"))
    }

    @Test
    fun onlyTheMacsOwnCertificateIsAccepted() {
        val mac = certificate(TestCertificates.MAC)
        val other = certificate(TestCertificates.OTHER)
        val trust = PinnedTrustManager(fingerprintOf(mac.encoded))
        trust.checkServerTrusted(arrayOf(mac), "ECDHE_ECDSA")
        // Another certificate, though just as well made, is somebody else.
        assertThrows(CertificateException::class.java) { trust.checkServerTrusted(arrayOf(other), "ECDHE_ECDSA") }
        // The Mac's certificate further down the chain does not count: the first one is who is talking.
        assertThrows(CertificateException::class.java) { trust.checkServerTrusted(arrayOf(other, mac), "ECDHE_ECDSA") }
        assertThrows(CertificateException::class.java) { trust.checkServerTrusted(emptyArray(), "ECDHE_ECDSA") }
        assertThrows(CertificateException::class.java) { trust.checkServerTrusted(null, "ECDHE_ECDSA") }
        assertThrows(CertificateException::class.java) { trust.checkClientTrusted(arrayOf(mac), "ECDHE_ECDSA") }
        assertEquals(0, trust.acceptedIssuers.size)
        // A fingerprint in capitals pins the same certificate.
        PinnedTrustManager(fingerprintOf(mac.encoded).uppercase()).checkServerTrusted(arrayOf(mac), "ECDHE_ECDSA")
        assertThrows(IllegalArgumentException::class.java) { PinnedTrustManager("not a fingerprint") }
    }

    @Test
    fun whileConnectingTheFirstCertificateIsTakenAndNoOtherAfterIt() {
        val mac = certificate(TestCertificates.MAC)
        val other = certificate(TestCertificates.OTHER)
        val trust = FirstSeenTrustManager()
        assertNull(trust.fingerprint)
        assertThrows(CertificateException::class.java) { trust.checkServerTrusted(emptyArray(), "ECDHE_ECDSA") }
        assertNull("showing nothing does not count as a first certificate", trust.fingerprint)
        trust.checkServerTrusted(arrayOf(mac), "ECDHE_ECDSA")
        assertEquals(fingerprintOf(mac.encoded), trust.fingerprint)
        // A second connection in the same attempt must be with the same certificate.
        trust.checkServerTrusted(arrayOf(mac), "ECDHE_ECDSA")
        assertThrows(CertificateException::class.java) { trust.checkServerTrusted(arrayOf(other), "ECDHE_ECDSA") }
        assertEquals(fingerprintOf(mac.encoded), trust.fingerprint)
        assertThrows(CertificateException::class.java) { trust.checkClientTrusted(arrayOf(mac), "ECDHE_ECDSA") }
    }

    // --- What a Mac announces ---

    @Test
    fun announcedAddressesAreOrderedAndTheUnusableOnesDropped() {
        val hosts = listOf(
            InetAddress.getByName("fe80::1"), // good on one interface only
            InetAddress.getByName("fd00::12"),
            InetAddress.getByName("192.168.1.20"),
            InetAddress.getByName("::1"),
        )
        assertEquals(listOf("192.168.1.20:47801", "[fd00:0:0:0:0:0:0:12]:47801"), usableAddresses(hosts, 47801))
        assertEquals(emptyList<String>(), usableAddresses(hosts, 0))
    }

    @Test
    fun anAnnouncedMacIsMatchedByTheStartOfItsFingerprint() {
        val fingerprint = "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        assertTrue(FoundMac("Mac", listOf("a:1"), "ba7816bf8f01cfea").couldBe(fingerprint))
        assertTrue(FoundMac("Mac", listOf("a:1"), "BA7816BF8F01CFEA").couldBe(fingerprint))
        assertFalse(FoundMac("Mac", listOf("a:1"), "ca7816bf8f01cfea").couldBe(fingerprint))
        // Too short a start, or none, proves nothing.
        assertFalse(FoundMac("Mac", listOf("a:1"), "ba78").couldBe(fingerprint))
        assertFalse(FoundMac("Mac", listOf("a:1"), null).couldBe(fingerprint))

        // What is worth trying as the connected Mac: the same fingerprint start
        // under any name, or the same name when no fingerprint is announced.
        val mac = PairedMac("MacBook", fingerprint, "192.168.1.20:47801", "device-1")
        assertTrue(FoundMac("Renamed since", listOf("a:1"), "ba7816bf8f01cfea").mayBe(mac))
        assertTrue(FoundMac("MacBook", listOf("a:1"), null).mayBe(mac))
        assertFalse(FoundMac("Other Mac", listOf("a:1"), null).mayBe(mac))
        // The same name with another certificate is another Mac.
        assertFalse(FoundMac("MacBook", listOf("a:1"), "0000000000000000").mayBe(mac))
    }

    // --- The connection, step by step ---

    @Test
    fun theConnectionFollowsWhatHappens() {
        var state: Connection = Connection.NotPaired
        // Nothing but connecting with a code leads out of "not connected".
        for (event in listOf(LinkEvent.CheckStarted, LinkEvent.Reached, LinkEvent.TurnedDown, LinkEvent.NoAnswer)) {
            assertEquals(Connection.NotPaired, state.after(event))
        }
        state = state.after(LinkEvent.Paired("MacBook"))
        assertEquals(Connection.Searching("MacBook"), state)
        state = state.after(LinkEvent.Reached)
        assertEquals(Connection.Connected("MacBook"), state)
        assertTrue(state.canSend)
        // A check behind a working connection is not shown.
        assertEquals(Connection.Connected("MacBook"), state.after(LinkEvent.CheckStarted))
        state = state.after(LinkEvent.NoAnswer)
        assertEquals(Connection.NotFound("MacBook"), state)
        assertFalse(state.canSend)
        state = state.after(LinkEvent.CheckStarted)
        assertEquals(Connection.Searching("MacBook"), state)
        state = state.after(LinkEvent.TurnedDown)
        assertEquals(Connection.Refused("MacBook"), state)
        // A late "no answer" does not hide that the Mac refused.
        assertEquals(Connection.Refused("MacBook"), state.after(LinkEvent.NoAnswer))
        // Connecting again, even with another Mac, starts afresh.
        assertEquals(Connection.Searching("iMac"), state.after(LinkEvent.Paired("iMac")))
        assertEquals(Connection.NotPaired, state.after(LinkEvent.Forgotten))
    }

    @Test
    fun theDebuggingConnectionIsItsOwnState() {
        val dev = Connection.Development(Link.Connected)
        assertTrue(dev.canSend)
        assertFalse(Connection.Development(Link.WrongToken).canSend)
        assertNull(dev.macName)
        assertEquals(dev, dev.after(LinkEvent.NoAnswer))
        assertEquals(dev, dev.after(LinkEvent.Forgotten))
        assertEquals(Connection.Searching("MacBook"), dev.after(LinkEvent.Paired("MacBook")))
    }

    // --- What is kept about the Mac ---

    @Test
    fun theStoreKeepsTheMacAndSealsTheToken() {
        val preferences = FakePreferences()
        val store = PairingStore(preferences, FakeBox())
        assertNull(store.load())
        val fingerprint = fingerprintOf("a certificate".toByteArray())
        val mac = PairedMac("MacBook", fingerprint, "192.168.1.20:47801", "device-1")
        store.save(mac, "the-secret-token")
        assertEquals(mac to "the-secret-token", store.load())
        assertFalse("the token is not stored as it is", preferences.values.values.any { it == "the-secret-token" })
        store.updateAddress("192.168.1.31:47801")
        assertEquals("192.168.1.31:47801", store.load()?.first?.address)
        // A seal that cannot be opened, as after the device's key was lost, means there is no connection.
        preferences.values["token_sealed"] = "garbage"
        assertNull(store.load())
        store.save(mac, "the-secret-token")
        store.clear()
        assertNull(store.load())
        assertTrue(preferences.values.isEmpty())
        // An address for a Mac that is not there is not kept.
        store.updateAddress("192.168.1.31:47801")
        assertTrue(preferences.values.isEmpty())
    }
}
