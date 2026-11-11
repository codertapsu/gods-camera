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
 */
class HubClient(baseUrl: String, private val token: String) {
    private val base = baseUrl.trim().trimEnd('/')
    private val json = Json { ignoreUnknownKeys = true }
    private val http = OkHttpClient.Builder()
        .connectTimeout(4, TimeUnit.SECONDS)
        .readTimeout(60, TimeUnit.SECONDS)
        .writeTimeout(60, TimeUnit.SECONDS)
        .build()

    private fun request(path: String) = Request.Builder().url("$base$path").header("Authorization", "Bearer $token")

    private suspend fun send(request: Request): String = withContext(Dispatchers.IO) {
        http.newCall(request).execute().use { response ->
            val body = response.body.string()
            if (!response.isSuccessful) throw HubException(response.code, body.take(200))
            body
        }
    }

    /** Public: answers without a token, so it tells "reachable" apart from "allowed". */
    suspend fun health(): Health = json.decodeFromString(send(Request.Builder().url("$base/v1/health").build()))

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
