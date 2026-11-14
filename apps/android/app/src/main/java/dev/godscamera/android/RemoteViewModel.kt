package dev.godscamera.android

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.Canvas
import android.graphics.LinearGradient
import android.graphics.Paint
import android.graphics.Shader
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.lifecycle.ViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.serialization.json.buildJsonObject
import kotlinx.serialization.json.put
import java.io.IOException

/** The tabs at the foot of the screen. */
enum class Tab { Remote, Scan }

/** Whether the computer's camera can be worked from here. */
enum class Reach { Unknown, Available, NotConnected, NotOnThisComputer, Unreachable }

/** What the Remote tab shows. */
data class RemoteUi(
    /** How the camera stands; `null` until the computer first answered. */
    val state: RemoteState? = null,
    /** The picture others see, a few times a second. */
    val picture: ImageBitmap? = null,
    val faces: List<RemoteFace> = emptyList(),
    val facePictures: Map<String, ImageBitmap> = emptyMap(),
    val reach: Reach = Reach.Unknown,
    /** What went wrong with the last thing asked, as a string resource, and a detail where there is no resource. */
    val message: Int? = null,
    val messageDetail: String? = null,
    /** Something was asked and the computer has not answered yet. */
    val asking: Boolean = false,
)

/**
 * The Remote tab's model. While the tab is in view and the app in front, it
 * asks the computer how its camera stands and for the picture others see,
 * a few times a second (api/remote.md), and passes the controls on: the face
 * swap, the face, recording a loop, and playing it.
 */
class RemoteViewModel : ViewModel() {
    private val _ui = MutableStateFlow(RemoteUi())
    val ui: StateFlow<RemoteUi> = _ui

    /** The tab in view. */
    val tab = MutableStateFlow(Tab.Remote)

    /** Where the hub is: the computer this device is connected to, or the debugging connection. */
    private var hub: () -> HubClient? = { null }

    /** A sample is shown, for pictures of the app; nothing is asked of a computer. */
    private var sample = false

    fun attach(hub: () -> HubClient?) {
        this.hub = hub
    }

    /**
     * Follow the computer's camera until the caller stops: the screen runs this
     * while it is in view and the app is in front, so nothing is asked otherwise.
     */
    suspend fun follow() {
        var round = 0
        while (true) {
            if (sample) return
            val client = hub()
            if (client == null) {
                _ui.update { it.copy(reach = Reach.NotConnected) }
                delay(1500)
                continue
            }
            try {
                val state = client.camera()
                _ui.update { it.copy(state = state, reach = Reach.Available) }
                // The faces at first, every ten seconds, and when the computer shows one this device does not know.
                val known = _ui.value.faces
                if (round % 25 == 0 || (state.selected_face != null && known.none { it.id == state.selected_face })) {
                    loadFaces(client)
                }
                try {
                    val jpeg = client.cameraPicture()
                    val picture = withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(jpeg, 0, jpeg.size)?.asImageBitmap() }
                    if (picture != null) _ui.update { it.copy(picture = picture) }
                } catch (none: HubException) {
                    // No picture yet: the camera is off on the computer. The state says so.
                    if (none.status != 404) throw none
                    _ui.update { it.copy(picture = null) }
                }
            } catch (refused: HubException) {
                val reach = if (refused.status == 404) Reach.NotOnThisComputer else Reach.Unreachable
                _ui.update { it.copy(reach = reach) }
                delay(2000)
            } catch (lost: IOException) {
                _ui.update { it.copy(reach = Reach.Unreachable) }
                delay(2000)
            }
            round++
            delay(400)
        }
    }

    private suspend fun loadFaces(client: HubClient) {
        val faces = client.cameraFaces()
        _ui.update { it.copy(faces = faces) }
        for (face in faces) {
            if (_ui.value.facePictures.containsKey(face.id)) continue
            val picture = runCatching {
                val png = client.facePicture(face.id)
                withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(png, 0, png.size)?.asImageBitmap() }
            }.getOrNull() ?: continue
            _ui.update { it.copy(facePictures = it.facePictures + (face.id to picture)) }
        }
    }

    fun swap(on: Boolean) = ask(buildJsonObject { put("action", "swap"); put("on", on) }.toString())

    fun chooseFace(id: String) = ask(buildJsonObject { put("action", "face"); put("id", id) }.toString())

    fun record() = ask("""{"action":"record"}""")

    fun loop() = ask("""{"action":"loop"}""")

    fun dismissMessage() = _ui.update { it.copy(message = null, messageDetail = null) }

    private fun ask(action: String) {
        if (sample) return
        val client = hub() ?: return
        viewModelScope.launch {
            _ui.update { it.copy(asking = true, message = null, messageDetail = null) }
            try {
                client.cameraDo(action)
                val state = client.camera()
                _ui.update { it.copy(state = state) }
            } catch (refused: HubException) {
                val code = refused.code
                val words = code?.let(::remoteMessage)
                _ui.update { it.copy(message = words ?: R.string.remote_failed, messageDetail = if (words == null) code ?: refused.message else null) }
            } catch (lost: IOException) {
                _ui.update { it.copy(message = R.string.remote_unreachable) }
            } finally {
                _ui.update { it.copy(asking = false) }
            }
        }
    }

    /** A made-up computer, for pictures of the app: nothing is asked of a real one. */
    fun showSample() {
        sample = true
        tab.value = Tab.Remote
        fun drawn(width: Int, height: Int, from: Int, to: Int, face: Boolean): ImageBitmap {
            val bitmap = Bitmap.createBitmap(width, height, Bitmap.Config.ARGB_8888)
            val canvas = Canvas(bitmap)
            canvas.drawPaint(Paint().apply { shader = LinearGradient(0f, 0f, width.toFloat(), height.toFloat(), from, to, Shader.TileMode.CLAMP) })
            if (face) {
                val light = Paint().apply { color = 0x8CFFFFFF.toInt(); isAntiAlias = true }
                canvas.drawOval(width * 0.38f, height * 0.18f, width * 0.62f, height * 0.62f, light)
                canvas.drawOval(width * 0.22f, height * 0.62f, width * 0.78f, height * 1.4f, light)
            }
            return bitmap.asImageBitmap()
        }
        val faces = listOf(RemoteFace("a", "Sample: me"), RemoteFace("b", "Sample: character"))
        _ui.value = RemoteUi(
            state = RemoteState(mode = "live", can_record = true, can_loop = true, swap_wanted = true, swapping = true, selected_face = "a", watched = true),
            picture = drawn(640, 360, 0xFF26A69A.toInt(), 0xFF3F51B5.toInt(), face = true),
            faces = faces,
            facePictures = mapOf(
                "a" to drawn(96, 96, 0xFF26C6DA.toInt(), 0xFF5C6BC0.toInt(), face = true),
                "b" to drawn(96, 96, 0xFFAB47BC.toInt(), 0xFF42A5F5.toInt(), face = true),
            ),
            reach = Reach.Available,
        )
    }
}
