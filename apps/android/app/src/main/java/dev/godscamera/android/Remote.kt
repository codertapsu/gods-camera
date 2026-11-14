package dev.godscamera.android

import kotlinx.serialization.Serializable

/**
 * The computer's camera as the hub describes it to a device (api/remote.md):
 * what the Camera screen on the computer shows, so that this device can work
 * the same controls during a call.
 */
@Serializable
data class RemoteState(
    /** `live`, `looping` or `idle`. */
    val mode: String = "idle",
    val recording: Boolean = false,
    val recorded_seconds: Double = 0.0,
    val can_record: Boolean = false,
    val can_loop: Boolean = false,
    val swap_wanted: Boolean = false,
    val swapping: Boolean = false,
    /** Why the swap is not on the face, as a code: see [remoteMessage]. */
    val swap_problem: String? = null,
    val selected_face: String? = null,
    /** An app on the computer reads the camera: someone in a call sees this picture. */
    val watched: Boolean = false,
) {
    val looping: Boolean get() = mode == "looping"
}

/** A face the computer can show. Its photos stay on the computer. */
@Serializable
data class RemoteFace(val id: String, val name: String)

/**
 * The words for a code the computer sends when it cannot do what was asked,
 * or says why the swap is not on the face. A code this app does not know
 * gets the general words, with the code itself.
 */
fun remoteMessage(code: String): Int? = when (code) {
    "accept_rules_on_computer" -> R.string.remote_accept_rules
    "add_a_face_first" -> R.string.remote_add_a_face
    "no_such_face" -> R.string.remote_no_such_face
    "cannot_record_now" -> R.string.remote_cannot_record
    "no_loop_yet" -> R.string.remote_no_loop
    "getting_ready" -> R.string.remote_getting_ready
    "model_missing" -> R.string.remote_model_missing
    "failed" -> R.string.remote_swap_failed
    "no_face_seen" -> R.string.remote_no_face_seen
    "not_on_face" -> R.string.remote_not_on_face
    else -> null
}

/** The codes the computer may send, as api/remote.md lists them. */
val remoteCodes = listOf(
    "accept_rules_on_computer", "add_a_face_first", "no_such_face", "cannot_record_now", "no_loop_yet",
    "getting_ready", "model_missing", "failed", "no_face_seen", "not_on_face",
)
