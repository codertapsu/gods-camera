package dev.godscamera.android

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import android.net.Uri
import android.os.Build
import android.provider.Settings
import androidx.core.content.edit
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job as CoroutineJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody
import okio.BufferedSink
import okio.source
import java.io.IOException

/** Why pages could not be sent or read. */
enum class SendProblem {
    /** The Mac did not answer, or the connection broke on the way. */
    MacUnreachable,

    /** The Mac no longer accepts this device. */
    MacRefused,

    /** The Mac took the pages and could not make a document of them. */
    ReadingFailed,
}

sealed interface Phase {
    data object Idle : Phase
    data class Uploading(val page: Int, val of: Int) : Phase
    data class Reading(val percent: Int) : Phase
    data class Done(val text: String, val pictures: Int, val lines: Int, val repeated: Int) : Phase
    data class Failed(val problem: SendProblem, val detail: String? = null) : Phase

    val isBusy: Boolean get() = this is Uploading || this is Reading
}

/** Why connecting with a code did not work. */
enum class PairProblem { WrongCode, NotOffering, Unreachable, TooManyDevices, BadAddress }

/** The steps of connecting to a Mac. */
sealed interface PairFlow {
    /** Not connecting at the moment. */
    data object Hidden : PairFlow

    /** Choosing a Mac from the ones in sight, or typing where it is. */
    data class Choosing(
        val typing: Boolean = false,
        val address: String = "",
        /** The typed address is being tried. */
        val checking: Boolean = false,
        val problem: PairProblem? = null,
    ) : PairFlow

    /** Entering the code the Mac shows. */
    data class Code(
        val macName: String,
        /** Where the Mac listens, the most usable address first. */
        val addresses: List<String>,
        val digits: String = "",
        val working: Boolean = false,
        val problem: PairProblem? = null,
    ) : PairFlow

    data class Done(val macName: String) : PairFlow
}

data class ScanState(
    val connection: Connection = Connection.NotPaired,
    val pages: List<Uri> = emptyList(),
    val phase: Phase = Phase.Idle,
    val pairing: PairFlow = PairFlow.Hidden,
    /** Macs in sight on the network, while the app is in front. */
    val macs: List<FoundMac> = emptyList(),
    val settingsOpen: Boolean = false,
)

/** The app's state and the work behind its buttons. */
class ScanViewModel(app: Application) : AndroidViewModel(app) {
    /** The debugging connection's address and token. Used by debug builds only. */
    private val devSettings = app.getSharedPreferences("hub", 0)
    private val finder = MacFinder(app)
    private val link = MacLink(viewModelScope, PairingStore(app.getSharedPreferences("pairing", 0), KeystoreBox()), finder.found)
    private val exchange = PairingExchange()
    private val connectivity = app.getSystemService(ConnectivityManager::class.java)

    /**
     * Debug builds: the token that was handed over through the debugging
     * connection in this run of the app. It is not kept: the next time the app
     * is opened by hand, it is an ordinary app that connects with a code.
     */
    private var devToken: String? = null
    private var devLink = Link.Unknown

    /**
     * Debug builds: the hub is reached through the debugging connection, not
     * as a connected Mac. That happens only in a run of the app in which a
     * token was handed over, until the app is closed or a Mac is connected
     * with a code.
     */
    private val devMode: Boolean
        get() = BuildConfig.DEBUG && devToken != null

    private val _state = MutableStateFlow(ScanState())
    val state: StateFlow<ScanState> = _state
    private var work: CoroutineJob? = null
    private var pairingWork: CoroutineJob? = null
    private var inFront = false

    /** Debug builds: the list of Macs holds made-up names, for pictures of the app. */
    private var showingSampleMacs = false

    private val networkWatch = object : ConnectivityManager.NetworkCallback() {
        override fun onAvailable(network: Network) = networkChanged()
        override fun onLost(network: Network) = networkChanged()
    }

    init {
        // Earlier versions kept the debugging token between runs, and then
        // greeted their user with a failed debugging connection. It is gone now.
        if (devSettings.contains("token")) devSettings.edit { remove("token") }
        link.begin()
        viewModelScope.launch { link.connection.collect { publishConnection() } }
        viewModelScope.launch { finder.found.collect { macs -> if (!showingSampleMacs) _state.update { it.copy(macs = macs) } } }
    }

    private fun publishConnection() {
        val connection = if (devMode) Connection.Development(devLink) else link.connection.value
        _state.update { it.copy(connection = connection) }
    }

    private fun networkChanged() {
        viewModelScope.launch { if (devMode) checkDevLink() else link.recheck() }
    }

    /** The app came to the front or left it. Macs are looked for only while it is in front. */
    fun setInFront(front: Boolean) {
        if (front == inFront) return
        inFront = front
        if (front) {
            finder.start()
            runCatching { connectivity?.registerDefaultNetworkCallback(networkWatch) }
            if (devMode) checkDevLink() else link.recheck()
        } else {
            finder.stop()
            runCatching { connectivity?.unregisterNetworkCallback(networkWatch) }
        }
    }

    override fun onCleared() {
        setInFront(false)
    }

    /** Look for the Mac again, because the person asked. */
    fun retryConnection() {
        if (devMode) checkDevLink() else link.recheck()
    }

    // --- Connecting with a code ---

    /** What the Mac lists this device as: the name its owner gave it, or its make and model. */
    private fun deviceName(): String {
        val app = getApplication<Application>()
        val given = runCatching { Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME) }.getOrNull()
        return given?.takeIf { it.isNotBlank() }
            ?: listOf(Build.MANUFACTURER.replaceFirstChar(Char::uppercase), Build.MODEL).joinToString(" ").trim().ifBlank { "Android" }
    }

    fun openConnect() {
        _state.update { it.copy(pairing = PairFlow.Choosing(), settingsOpen = false) }
    }

    fun closeConnect() {
        pairingWork?.cancel()
        _state.update { it.copy(pairing = PairFlow.Hidden) }
    }

    /** One step back in the connect flow; from its first step, out of it. */
    fun backInConnect() {
        pairingWork?.cancel()
        _state.update {
            when (val flow = it.pairing) {
                is PairFlow.Code -> it.copy(pairing = PairFlow.Choosing())
                is PairFlow.Choosing -> if (flow.typing) it.copy(pairing = PairFlow.Choosing()) else it.copy(pairing = PairFlow.Hidden)
                else -> it.copy(pairing = PairFlow.Hidden)
            }
        }
    }

    fun chooseMac(mac: FoundMac) {
        _state.update { it.copy(pairing = PairFlow.Code(mac.name, mac.addresses)) }
    }

    fun typeAddressInstead() {
        _state.update { it.copy(pairing = PairFlow.Choosing(typing = true)) }
    }

    fun setTypedAddress(text: String) {
        _state.update {
            val flow = it.pairing as? PairFlow.Choosing ?: return@update it
            it.copy(pairing = flow.copy(address = text, problem = null))
        }
    }

    /** Try the typed address: if a Mac answers there, go on to the code. */
    fun submitTypedAddress() {
        val flow = _state.value.pairing as? PairFlow.Choosing ?: return
        val address = parseMacAddress(flow.address)
        if (address == null) {
            _state.update { it.copy(pairing = flow.copy(problem = PairProblem.BadAddress)) }
            return
        }
        _state.update { it.copy(pairing = flow.copy(checking = true, problem = null)) }
        pairingWork?.cancel()
        pairingWork = viewModelScope.launch {
            val hello = exchange.hello(address)
            _state.update {
                if (it.pairing !is PairFlow.Choosing) return@update it
                if (hello == null) it.copy(pairing = flow.copy(checking = false, problem = PairProblem.Unreachable))
                else it.copy(pairing = PairFlow.Code(hello.name.ifBlank { address }, listOf(address)))
            }
        }
    }

    /** The code field changed. Six digits are tried at once, without a button. */
    fun setCode(typed: String) {
        val flow = _state.value.pairing as? PairFlow.Code ?: return
        if (flow.working) return
        val digits = codeDigits(typed)
        _state.update { it.copy(pairing = flow.copy(digits = digits, problem = if (digits == flow.digits) flow.problem else null)) }
        if (isCompleteCode(digits) && digits != flow.digits) submitCode()
    }

    fun submitCode() {
        val flow = _state.value.pairing as? PairFlow.Code ?: return
        if (flow.working || !isCompleteCode(flow.digits)) return
        _state.update { it.copy(pairing = flow.copy(working = true, problem = null)) }
        pairingWork?.cancel()
        pairingWork = viewModelScope.launch {
            // A Mac may announce several addresses; the first one that answers decides.
            var outcome: PairOutcome = PairOutcome.Unreachable
            for (address in flow.addresses) {
                outcome = exchange.pair(address, flow.macName, flow.digits, deviceName())
                if (outcome != PairOutcome.Unreachable) break
            }
            when (val result = outcome) {
                is PairOutcome.Paired -> {
                    devToken = null
                    link.pairedWith(result.mac, result.token)
                    _state.update { it.copy(pairing = PairFlow.Done(result.mac.name)) }
                }
                else -> {
                    val problem = when (result) {
                        PairOutcome.NotOffering -> PairProblem.NotOffering
                        PairOutcome.TooManyDevices -> PairProblem.TooManyDevices
                        PairOutcome.Unreachable -> PairProblem.Unreachable
                        else -> PairProblem.WrongCode
                    }
                    // The digits are cleared: the same code will not work a second time.
                    _state.update {
                        if (it.pairing is PairFlow.Code) it.copy(pairing = flow.copy(digits = "", working = false, problem = problem)) else it
                    }
                }
            }
        }
    }

    // --- Settings ---

    fun openSettings() = _state.update { it.copy(settingsOpen = true) }

    fun closeSettings() = _state.update { it.copy(settingsOpen = false) }

    /** The Mac this device is connected to, for the settings sheet. */
    val pairedMac: PairedMac? get() = link.mac

    fun forgetMac() {
        link.forget()
        publishConnection()
        if (devMode) checkDevLink()
        _state.update { it.copy(settingsOpen = false) }
    }

    // --- The debugging connection (debug builds) ---

    /** Debug builds: take the hub's token from the debugging connection and use the hub through it. */
    fun useDevelopmentHub(token: String) {
        if (!BuildConfig.DEBUG) return
        devToken = token.trim()
        publishConnection()
        checkDevLink()
    }

    private fun devClient() = HubClient(
        // Through `adb reverse`, the Mac's hub is this device's own localhost.
        devSettings.getString("address", null) ?: "http://127.0.0.1:47800",
        devToken ?: "",
        HubHttp.plain(),
    )

    /** Is the hub there, and does it accept the token? */
    private fun checkDevLink() {
        viewModelScope.launch {
            devLink = try {
                val hub = devClient()
                hub.health()
                // An authorised call that changes nothing: asking for an upload that does not exist.
                try {
                    hub.upload("00000000-0000-0000-0000-000000000000")
                    Link.Connected
                } catch (refused: HubException) {
                    if (refused.status == 401 || refused.status == 403) Link.WrongToken else Link.Connected
                }
            } catch (_: IOException) {
                Link.Unreachable
            } catch (_: IllegalArgumentException) {
                Link.Unreachable
            }
            publishConnection()
        }
    }

    /** Debug builds: connect with a code without the screens, as `adb` hands it over. */
    fun debugPair(address: String, code: String) {
        if (!BuildConfig.DEBUG) return
        val parsed = parseMacAddress(address) ?: return
        pairingWork?.cancel()
        pairingWork = viewModelScope.launch {
            val name = exchange.hello(parsed)?.name?.ifBlank { null } ?: parsed
            _state.update { it.copy(pairing = PairFlow.Code(name, listOf(parsed), codeDigits(code))) }
            submitCode()
        }
    }

    /** Debug builds: send the pages as soon as there is a Mac to send them to. */
    fun debugSendWhenReady() {
        if (!BuildConfig.DEBUG) return
        viewModelScope.launch {
            withTimeoutOrNull(30_000) { state.first { it.connection.canSend && it.pages.isNotEmpty() && it.pairing !is PairFlow.Code } }
                ?: return@launch
            if (_state.value.pairing is PairFlow.Done) closeConnect()
            send()
        }
    }

    /** Debug builds: show one of the app's screens, for pictures of it. */
    fun debugShow(screen: String) {
        if (!BuildConfig.DEBUG) return
        showingSampleMacs = screen == "connect-sample"
        // One screen at a time: whatever was open before is closed.
        _state.update { it.copy(pairing = PairFlow.Hidden, settingsOpen = false) }
        when (screen) {
            "connect" -> openConnect()
            "connect-sample" -> _state.update {
                it.copy(
                    pairing = PairFlow.Choosing(),
                    macs = listOf(FoundMac("MacBook Pro", emptyList(), null), FoundMac("Mac mini", emptyList(), null)),
                )
            }
            "address" -> _state.update { it.copy(pairing = PairFlow.Choosing(typing = true)) }
            "code" -> _state.update { it.copy(pairing = PairFlow.Code("MacBook Pro", emptyList(), digits = "482")) }
            "wrong-code" -> _state.update { it.copy(pairing = PairFlow.Code("MacBook Pro", emptyList(), problem = PairProblem.WrongCode)) }
            "connected" -> _state.update { it.copy(pairing = PairFlow.Done("MacBook Pro")) }
            "settings" -> openSettings()
        }
    }

    // --- Pages ---

    fun addPages(uris: List<Uri>) {
        if (uris.isEmpty()) return
        _state.update { it.copy(pages = it.pages + uris, phase = if (it.phase.isBusy) it.phase else Phase.Idle) }
    }

    fun removePage(index: Int) {
        if (_state.value.phase.isBusy) return
        _state.update { it.copy(pages = it.pages.filterIndexed { at, _ -> at != index }, phase = Phase.Idle) }
    }

    fun clearPages() {
        work?.cancel()
        _state.update { it.copy(pages = emptyList(), phase = Phase.Idle) }
    }

    /** Leave a failure behind and show the pages again. */
    fun dismissProblem() {
        _state.update { if (it.phase is Phase.Failed) it.copy(phase = Phase.Idle) else it }
    }

    private fun client(): HubClient? = if (devMode) devClient() else link.client()

    /** Upload the pages, have the Mac read and join them, and fetch the text. */
    fun send() {
        val pages = _state.value.pages
        if (pages.isEmpty() || work?.isActive == true) return
        val hub = client()
        if (hub == null) {
            _state.update { it.copy(phase = Phase.Failed(SendProblem.MacUnreachable)) }
            return
        }
        work = viewModelScope.launch {
            try {
                val upload = hub.createUpload()
                pages.forEachIndexed { index, uri ->
                    _state.update { it.copy(phase = Phase.Uploading(index + 1, pages.size)) }
                    sendPage(hub, upload.id, index, uri)
                }
                var job = hub.startJob("document", upload.id)
                while (job.status == "queued" || job.status == "running") {
                    _state.update { it.copy(phase = Phase.Reading((job.progress * 100).toInt())) }
                    delay(300)
                    job = hub.job(job.id)
                }
                if (job.status != "done") {
                    _state.update { it.copy(phase = Phase.Failed(SendProblem.ReadingFailed, job.error)) }
                    return@launch
                }
                val text = hub.jobFile(job.id, "document.txt")
                fun count(key: String) = job.result?.get(key)?.jsonPrimitive?.int ?: 0
                _state.update {
                    it.copy(phase = Phase.Done(text, count("pictures"), count("lines_read"), count("repeated_lines_removed")))
                }
            } catch (refused: HubException) {
                if (refused.status == 401) {
                    // The Mac no longer knows this device: say so where the connection is shown, too.
                    if (devMode) checkDevLink() else link.turnedDown()
                    _state.update { it.copy(phase = Phase.Failed(SendProblem.MacRefused)) }
                } else {
                    _state.update { it.copy(phase = Phase.Failed(SendProblem.ReadingFailed, refused.message)) }
                }
            } catch (_: IOException) {
                _state.update { it.copy(phase = Phase.Failed(SendProblem.MacUnreachable)) }
                retryConnection()
            } catch (_: IllegalArgumentException) {
                _state.update { it.copy(phase = Phase.Failed(SendProblem.MacUnreachable)) }
            }
        }
    }

    /**
     * Send one picture. If the connection drops part-way, the hub keeps what
     * it received, and the next try carries on from there.
     */
    private suspend fun sendPage(hub: HubClient, uploadId: String, index: Int, uri: Uri) {
        val resolver = getApplication<Application>().contentResolver
        val type = resolver.getType(uri) ?: "image/jpeg"
        val extension = when (type) {
            "image/png" -> "png"
            "image/webp" -> "webp"
            "image/heic", "image/heif" -> "heic"
            else -> "jpg"
        }
        val name = "page-%03d.%s".format(index + 1, extension)
        var lastFailure: IOException? = null
        repeat(3) {
            try {
                val stored = hub.upload(uploadId).files.firstOrNull { it.name == name }?.bytes ?: 0L
                hub.putFile(uploadId, name, stored, object : RequestBody() {
                    override fun contentType(): MediaType = type.toMediaType()
                    override fun writeTo(sink: BufferedSink) {
                        val input = resolver.openInputStream(uri) ?: throw IOException("the picture cannot be opened")
                        input.use { stream ->
                            var toSkip = stored
                            while (toSkip > 0) {
                                val skipped = stream.skip(toSkip)
                                if (skipped <= 0) break
                                toSkip -= skipped
                            }
                            sink.writeAll(stream.source())
                        }
                    }
                })
                return
            } catch (failure: HubException) {
                throw failure
            } catch (failure: IOException) {
                lastFailure = failure
                delay(500)
            }
        }
        throw lastFailure ?: IOException("the picture could not be sent")
    }
}
