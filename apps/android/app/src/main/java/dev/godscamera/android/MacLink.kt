package dev.godscamera.android

import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import java.io.IOException

/** One try at reaching the Mac at one address. */
enum class Answer { Accepted, Refused, Silent }

/**
 * Keeps this device connected to its Mac.
 *
 * The Mac may get a new address whenever it joins a network, so the address
 * is never taken for granted: the last one that worked is tried first, then
 * every address the Mac announces. Whatever answers must show the Mac's own
 * certificate, so a wrong address can do no harm.
 */
class MacLink(
    private val scope: CoroutineScope,
    private val store: PairingStore,
    private val found: StateFlow<List<FoundMac>>,
    /** Ask the Mac at an address whether it accepts this device. */
    private val ask: suspend (address: String, mac: PairedMac, token: String) -> Answer = ::askMac,
    /** How long to keep looking before saying the Mac was not found, in milliseconds. */
    private val patienceMs: Long = 8000,
) {
    private val _connection = MutableStateFlow<Connection>(Connection.NotPaired)
    val connection: StateFlow<Connection> = _connection
    private var paired: Pair<PairedMac, String>? = null
    private var check: Job? = null

    /** The Mac this device is connected to, if any. */
    val mac: PairedMac? get() = paired?.first

    /** A client for the Mac, when one is known. */
    fun client(): HubClient? = paired?.let { (mac, token) ->
        HubClient("https://${mac.address}", token, HubHttp.pinned(mac.fingerprint))
    }

    /** Read what was stored and start looking for the Mac. */
    fun begin() {
        paired = store.load()
        val mac = paired?.first
        _connection.value = if (mac == null) Connection.NotPaired else Connection.Searching(mac.name)
        if (mac != null) recheck()
        scope.launch {
            // A Mac that comes into sight after it was given up on is tried again.
            found.collect { macs ->
                val known = paired?.first ?: return@collect
                if (_connection.value is Connection.NotFound && macs.any { it.mayBe(known) }) recheck()
            }
        }
    }

    /** The code was accepted. */
    fun pairedWith(mac: PairedMac, token: String) {
        store.save(mac, token)
        paired = mac to token
        _connection.update { it.after(LinkEvent.Paired(mac.name)) }
        recheck()
    }

    fun forget() {
        check?.cancel()
        store.clear()
        paired = null
        _connection.update { Connection.NotPaired }
    }

    /** The Mac turned a request down as coming from a device it does not know. */
    fun turnedDown() {
        check?.cancel()
        _connection.update { it.after(LinkEvent.TurnedDown) }
    }

    /** Look for the Mac again: at start, when the network changes, or when the person asks. */
    fun recheck() {
        val (mac, token) = paired ?: return
        check?.cancel()
        check = scope.launch {
            _connection.update { it.after(LinkEvent.CheckStarted) }
            val tried = mutableSetOf<String>()
            var waited = 0L
            while (true) {
                val candidates = (listOf(mac.address) + found.value.filter { it.mayBe(mac) }.flatMap { it.addresses })
                    .filter { it !in tried }
                for (address in candidates) {
                    tried += address
                    when (ask(address, mac, token)) {
                        Answer.Accepted -> {
                            if (address != mac.address) {
                                store.updateAddress(address)
                                paired = mac.copy(address = address) to token
                            }
                            _connection.update { it.after(LinkEvent.Reached) }
                            return@launch
                        }
                        Answer.Refused -> {
                            _connection.update { it.after(LinkEvent.TurnedDown) }
                            return@launch
                        }
                        Answer.Silent -> {}
                    }
                }
                if (waited >= patienceMs) break
                // The Mac's announcement may still be on its way.
                delay(STEP_MS)
                waited += STEP_MS
            }
            _connection.update { it.after(LinkEvent.NoAnswer) }
        }
    }
}

/** How long to wait between looks at what the network has announced, in milliseconds. */
private const val STEP_MS = 400L

/** The real question: `GET /v1/health` over a connection that accepts only the Mac's certificate. */
suspend fun askMac(address: String, mac: PairedMac, token: String): Answer = try {
    HubClient("https://$address", token, HubHttp.pinned(mac.fingerprint)).healthQuickly()
    Answer.Accepted
} catch (refused: HubException) {
    if (refused.status == 401 || refused.status == 403) Answer.Refused else Answer.Silent
} catch (_: IOException) {
    Answer.Silent
} catch (_: IllegalArgumentException) {
    Answer.Silent
}
