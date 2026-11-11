package dev.godscamera.android

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import kotlinx.serialization.json.JsonObject
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody
import okhttp3.RequestBody.Companion.toRequestBody
import java.io.IOException
import java.util.concurrent.TimeUnit

@Serializable
data class Health(val status: String, val api: String, val mode: String)

@Serializable
data class FileInfo(val name: String, val bytes: Long)

@Serializable
data class UploadInfo(val id: String, val files: List<FileInfo> = emptyList())

@Serializable
data class Job(
    val id: String,
    val kind: String,
    val status: String,
    val progress: Float = 0f,
    val result: JsonObject? = null,
    val files: List<String> = emptyList(),
    val error: String? = null,
)

/** The hub said no. [status] is the HTTP status. */
class HubException(val status: Int, message: String) : IOException(message)

/**
 * The hub on the desktop, as this app talks to it. Every call runs off the
 * main thread and throws [IOException] when the hub cannot be reached and
 * [HubException] when it refuses.
 *
 * [http] decides whom to trust: `HubHttp.pinned` for a Mac this device is
 * connected to, `HubHttp.plain` for the debugging connection.
 */
class HubClient(baseUrl: String, private val token: String, private val http: OkHttpClient) {
    private val base = baseUrl.trim().trimEnd('/')
    private val json = Json { ignoreUnknownKeys = true }

    private fun request(path: String) = Request.Builder().url("$base$path").header("Authorization", "Bearer $token")

    private suspend fun send(request: Request): String = withContext(Dispatchers.IO) {
        http.newCall(request).execute().use { response ->
            val body = response.body.string()
            if (!response.isSuccessful) throw HubException(response.code, body.take(200))
            body
        }
    }

    /**
     * Is the hub there? Over the network it answers a device it knows and
     * refuses, with 401, one it has removed. Over the debugging connection it
     * answers anyone, so there the token is checked with a second call.
     */
    suspend fun health(): Health = json.decodeFromString(send(request("/v1/health").build()))

    /** [health] with a short wait, for trying one place after another. */
    suspend fun healthQuickly(): Health {
        val quick = http.newBuilder().connectTimeout(2500, TimeUnit.MILLISECONDS).readTimeout(4, TimeUnit.SECONDS).build()
        return withContext(Dispatchers.IO) {
            quick.newCall(request("/v1/health").build()).execute().use { response ->
                val body = response.body.string()
                if (!response.isSuccessful) throw HubException(response.code, body.take(200))
                json.decodeFromString(body)
            }
        }
    }

    suspend fun createUpload(): UploadInfo =
        json.decodeFromString(send(request("/v1/uploads").post(ByteArray(0).toRequestBody()).build()))

    suspend fun upload(id: String): UploadInfo = json.decodeFromString(send(request("/v1/uploads/$id").build()))

    /** Append to a file from [offset], which must be the number of bytes the hub already holds. */
    suspend fun putFile(uploadId: String, name: String, offset: Long, body: RequestBody): FileInfo =
        json.decodeFromString(send(request("/v1/uploads/$uploadId/files/$name?offset=$offset").put(body).build()))

    suspend fun startJob(kind: String, uploadId: String): Job {
        val body = """{"kind":"$kind","upload_id":"$uploadId"}""".toRequestBody("application/json".toMediaType())
        return json.decodeFromString(send(request("/v1/jobs").post(body).build()))
    }

    suspend fun job(id: String): Job = json.decodeFromString(send(request("/v1/jobs/$id").build()))

    suspend fun jobFile(id: String, name: String): String = send(request("/v1/jobs/$id/files/$name").build())
}
