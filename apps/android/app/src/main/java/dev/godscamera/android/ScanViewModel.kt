package dev.godscamera.android

import android.app.Application
import android.net.Uri
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Job as CoroutineJob
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.serialization.json.int
import kotlinx.serialization.json.jsonPrimitive
import okhttp3.MediaType
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.RequestBody
import okio.BufferedSink
import okio.source
import java.io.IOException

enum class Link { Unknown, Connected, WrongToken, Unreachable }

sealed interface Phase {
    data object Idle : Phase
    data class Uploading(val page: Int, val of: Int) : Phase
    data class Reading(val percent: Int) : Phase
    data class Done(val text: String, val pictures: Int, val lines: Int, val repeated: Int) : Phase
    data class Failed(val reason: String) : Phase
}

data class ScanState(
    val address: String,
    val token: String,
    val link: Link = Link.Unknown,
    val pages: List<Uri> = emptyList(),
    val phase: Phase = Phase.Idle,
)

/** The scanner screen's state and the work behind its buttons. */
class ScanViewModel(app: Application) : AndroidViewModel(app) {
    private val settings = app.getSharedPreferences("hub", 0)
    private val _state = MutableStateFlow(
        ScanState(
            // Through `adb reverse`, the Mac's hub is this device's own localhost.
            address = settings.getString("address", null) ?: "http://127.0.0.1:47800",
            token = settings.getString("token", null) ?: "",
        )
    )
    val state: StateFlow<ScanState> = _state
    private var work: CoroutineJob? = null

    private fun client() = HubClient(_state.value.address, _state.value.token)

    fun setAddress(value: String) {
        _state.update { it.copy(address = value, link = Link.Unknown) }
        settings.edit().putString("address", value).apply()
    }

    fun setToken(value: String) {
        _state.update { it.copy(token = value.trim(), link = Link.Unknown) }
        settings.edit().putString("token", value.trim()).apply()
    }

    fun addPages(uris: List<Uri>) {
        if (uris.isEmpty()) return
        _state.update { it.copy(pages = it.pages + uris, phase = Phase.Idle) }
    }

    fun clearPages() {
        work?.cancel()
        _state.update { it.copy(pages = emptyList(), phase = Phase.Idle) }
    }

    /** Is the hub there, and does it accept the token? */
    fun checkConnection() {
        viewModelScope.launch {
            val link = try {
                val hub = client()
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
                // The address is not a URL.
                Link.Unreachable
            }
            _state.update { it.copy(link = link) }
        }
    }

    /** Upload the pages, have the hub read and merge them, and fetch the text. */
    fun send() {
        val pages = _state.value.pages
        if (pages.isEmpty() || work?.isActive == true) return
        work = viewModelScope.launch {
            try {
                val hub = client()
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
                if (job.status != "done") throw IOException(job.error ?: "the job failed")
                val text = hub.jobFile(job.id, "document.txt")
                fun count(key: String) = job.result?.get(key)?.jsonPrimitive?.int ?: 0
                _state.update {
                    it.copy(phase = Phase.Done(text, count("pictures"), count("lines_read"), count("repeated_lines_removed")), link = Link.Connected)
                }
            } catch (failure: IOException) {
                _state.update { it.copy(phase = Phase.Failed(failure.message ?: failure.javaClass.simpleName)) }
            } catch (failure: IllegalArgumentException) {
                _state.update { it.copy(phase = Phase.Failed(failure.message ?: "bad address")) }
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
