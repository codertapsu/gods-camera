package dev.godscamera.android

import kotlinx.coroutines.runBlocking
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody.Companion.toRequestBody
import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Assume.assumeTrue
import org.junit.Before
import org.junit.Test
import java.io.File
import java.io.IOException
import java.net.ServerSocket
import java.nio.file.Files
import java.util.concurrent.LinkedBlockingQueue
import java.util.concurrent.TimeUnit

/**
 * The app's own code for connecting with a code and for talking to the Mac
 * afterwards, run against a real hub: the command-line tool from `core/`,
 * started on spare ports with a folder of its own.
 *
 * Needs `core/target/release/gc` and the shared core built for this computer
 * (`tools/build-core-android.sh` makes the latter; `cargo build --release -p
 * gc-cli` in `core/` the former). Without them the test is skipped. The hub's
 * network listener is open to the local network for the seconds this takes;
 * nothing can use it without the code.
 */
class PairingWithARealHubTest {
    private var hub: Process? = null
    private var data: File? = null
    private val lines = LinkedBlockingQueue<String>()
    private var lanPort = 0
    private var printedFingerprint = ""
    private var code = ""

    private fun sparePort(): Int = ServerSocket(0).use { it.localPort }

    /** Wait for a line of the hub's output that starts with [prefix], and return the rest of it. */
    private fun await(prefix: String): String {
        val deadline = System.nanoTime() + TimeUnit.SECONDS.toNanos(20)
        while (System.nanoTime() < deadline) {
            val line = lines.poll(200, TimeUnit.MILLISECONDS) ?: continue
            if (line.startsWith(prefix)) return line.removePrefix(prefix).trim()
        }
        fail("the hub never printed \"$prefix…\"")
        return ""
    }

    @Before
    fun startHub() {
        val tool = File(System.getProperty("gc.tool") ?: "")
        val core = File(System.getProperty("jna.library.path") ?: "", System.mapLibraryName("gc_mobile"))
        assumeTrue("the command-line tool is not built: ${tool.path}", tool.canExecute())
        assumeTrue("the shared core is not built for this computer: ${core.path}", core.isFile)
        // An older build of the tool, from before hubs could be connected to with a code, cannot run this.
        val help = ProcessBuilder(tool.path, "hub", "--help").redirectErrorStream(true).start()
        assumeTrue("the command-line tool is older than connecting with a code", help.inputStream.bufferedReader().readText().contains("--pair"))

        data = Files.createTempDirectory("gc-hub-test").toFile()
        lanPort = sparePort()
        val process = ProcessBuilder(
            tool.path, "hub", "--addr", "127.0.0.1:${sparePort()}", "--data-dir", data!!.path,
            "--lan", "--lan-port", "$lanPort", "--pair", "--name", "Test Mac",
        ).redirectErrorStream(true).apply { environment()["GC_HUB_TOKEN"] = "0123456789abcdef0123456789abcdef" }.start()
        hub = process
        Thread { process.inputStream.bufferedReader().forEachLine(lines::put) }.apply { isDaemon = true }.start()
        printedFingerprint = await("certificate fingerprint")
        // "pairing code 123 456 (good for 5 minutes, for one device)"
        code = codeDigits(await("pairing code").substringBefore("("))
        assertTrue(isCompleteCode(code))
    }

    @After
    fun stopHub() {
        hub?.destroy()
        hub?.waitFor(5, TimeUnit.SECONDS)
        data?.deleteRecursively()
    }

    @Test
    fun aDeviceConnectsWithTheCodeAndThenSendsOverAPinnedConnection() = runBlocking {
        val address = "127.0.0.1:$lanPort"
        val exchange = PairingExchange()

        // Before anything is proven, the Mac says its name and that it shows a code.
        val hello = assertNotNull(exchange.hello(address)).let { exchange.hello(address)!! }
        assertEquals("Test Mac", hello.name)
        assertTrue(hello.pairingOpen)
        assertNull("nothing listens there", exchange.hello("127.0.0.1:${sparePort()}"))

        // A wrong code is turned down, without saying more.
        val wrong = if (code == "000000") "000001" else "000000"
        assertEquals(PairOutcome.WrongCode, exchange.pair(address, hello.name, wrong, "Test tablet"))
        // A code that is not six digits never leaves the device.
        assertEquals(PairOutcome.WrongCode, exchange.pair(address, hello.name, "12", "Test tablet"))
        assertEquals(PairOutcome.Unreachable, exchange.pair("127.0.0.1:${sparePort()}", hello.name, code, "Test tablet"))

        // The right code connects, and what is kept names the hub's own certificate.
        val outcome = exchange.pair(address, hello.name, code, "  Test   tablet ")
        val paired = outcome as? PairOutcome.Paired ?: return@runBlocking fail("not connected: $outcome")
        assertEquals(printedFingerprint, paired.mac.fingerprint)
        assertEquals("Test Mac", paired.mac.name)
        assertEquals(address, paired.mac.address)
        assertTrue(paired.mac.deviceId.isNotBlank())
        assertEquals(64, paired.token.length)
        // The hub lists the device under its tidied name.
        assertEquals("Test tablet", await("paired with"))

        // The code was for one device: it is gone now.
        assertEquals(PairOutcome.NotOffering, exchange.pair(address, hello.name, code, "Second tablet"))
        assertEquals(false, exchange.hello(address)?.pairingOpen)

        // Afterwards: only that certificate is accepted, and the token opens the hub.
        assertEquals(Answer.Accepted, askMac(address, paired.mac, paired.token))
        val mac = HubClient("https://$address", paired.token, HubHttp.pinned(paired.mac.fingerprint))
        assertEquals("paired", mac.health().mode)
        assertEquals("paired", mac.healthQuickly().mode)

        // The whole way a page travels, with a job that needs no text reader.
        val upload = mac.createUpload()
        val bytes = ByteArray(3000) { (it % 251).toByte() }
        // In two parts, as after a connection that dropped half-way.
        mac.putFile(upload.id, "page-001.jpg", 0, bytes.copyOfRange(0, 1000).toRequestBody("image/jpeg".toMediaType()))
        assertEquals(1000L, mac.upload(upload.id).files.single().bytes)
        mac.putFile(upload.id, "page-001.jpg", 1000, bytes.copyOfRange(1000, 3000).toRequestBody("image/jpeg".toMediaType()))
        assertEquals(3000L, mac.upload(upload.id).files.single().bytes)
        var job = mac.startJob("inventory", upload.id)
        repeat(50) {
            if (job.status == "queued" || job.status == "running") {
                Thread.sleep(100)
                job = mac.job(job.id)
            }
        }
        assertEquals("done", job.status)

        // A wrong token is refused, and the same hub behind another certificate's name is not talked to at all.
        assertEquals(Answer.Refused, askMac(address, paired.mac, "f".repeat(64)))
        val stranger = paired.mac.copy(fingerprint = fingerprintOf("another certificate".toByteArray()))
        assertEquals(Answer.Silent, askMac(address, stranger, paired.token))
        try {
            HubClient("https://$address", paired.token, HubHttp.pinned(stranger.fingerprint)).health()
            fail("a hub with another certificate was accepted")
        } catch (_: IOException) {
            // As it should be: the connection is not even made.
        }
    }
}
