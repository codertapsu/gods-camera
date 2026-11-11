package dev.godscamera.android

import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.json.Json
import okhttp3.MediaType.Companion.toMediaType
import okhttp3.OkHttpClient
import okhttp3.Request
import okhttp3.RequestBody.Companion.toRequestBody
import uniffi.gc_mobile.PairingClient
import uniffi.gc_mobile.PairingException
import java.io.IOException
import java.util.Base64

/** A Mac this device is connected to. The token that goes with it is kept apart, encrypted. */
data class PairedMac(
    val name: String,
    /** The SHA-256 of the Mac's certificate. No other certificate is accepted. */
    val fingerprint: String,
    /** Where the Mac was last reached, as host:port. */
    val address: String,
    /** This device's id on the Mac. */
    val deviceId: String,
)

/** What a Mac says about itself before anything is proven. */
@Serializable
data class MacHello(val name: String, val version: String = "", val api: String = "", @SerialName("pairing_open") val pairingOpen: Boolean = false)

/** How an attempt to connect with a code ended. */
sealed interface PairOutcome {
    /** [token] is a secret: it goes to the token store and nowhere else. */
    class Paired(val mac: PairedMac, val token: String) : PairOutcome

    /** The code was wrong, or what answered is not the Mac that shows the code. The two cannot be told apart. */
    data object WrongCode : PairOutcome

    /** The Mac is not showing a code at the moment, or its code ran out. */
    data object NotOffering : PairOutcome

    /** The Mac has as many devices as it takes. */
    data object TooManyDevices : PairOutcome

    /** Nothing answered there, or what answered is not God's Camera. */
    data object Unreachable : PairOutcome
}

@Serializable
private data class StartRequest(@SerialName("device_name") val deviceName: String, val message: String)

@Serializable
private data class StartResponse(val session: String, val message: String)

@Serializable
private data class FinishRequest(val session: String, val confirmation: String)

@Serializable
private data class FinishResponse(val confirmation: String, val grant: String)

@Serializable
private data class Refusal(val error: String = "")

/**
 * Connects this device to a Mac with the six-digit code the Mac shows.
 *
 * The talking is done here; the arithmetic is the shared core's, the same
 * code the Mac runs on its side. Neither the code nor the token is ever logged.
 */
class PairingExchange {
    private val json = Json { ignoreUnknownKeys = true }
    private val jsonType = "application/json".toMediaType()

    private fun refusal(body: String): String = runCatching { json.decodeFromString<Refusal>(body).error }.getOrDefault("")

    /** Let go of a client that served its one attempt. */
    private fun OkHttpClient.discard() {
        connectionPool.evictAll()
        dispatcher.executorService.shutdown()
    }

    /**
     * Ask what is at [address] (host:port). Null if nothing answers or it is
     * not God's Camera. The answer is not proof of anything: it only says
     * what to call the Mac and whether trying a code makes sense.
     */
    suspend fun hello(address: String): MacHello? = withContext(Dispatchers.IO) {
        val (http, _) = HubHttp.unproven()
        try {
            http.newCall(Request.Builder().url("https://$address/v1/hello").build()).execute().use { response ->
                if (!response.isSuccessful) null else json.decodeFromString<MacHello>(response.body.string())
            }
        } catch (_: IOException) {
            null
        } catch (_: IllegalArgumentException) {
            null
        } finally {
            http.discard()
        }
    }

    /**
     * Run the exchange with the Mac at [address] (host:port). [macName] is the
     * name to remember it by; [deviceName] is what the Mac lists this device as.
     */
    suspend fun pair(address: String, macName: String, code: String, deviceName: String): PairOutcome =
        withContext(Dispatchers.IO) {
            val client = try {
                PairingClient(code, deviceName)
            } catch (_: PairingException.InvalidName) {
                PairingClient(code, "Android")
            } catch (_: PairingException) {
                return@withContext PairOutcome.WrongCode
            }
            val (http, trust) = HubHttp.unproven()
            try {
                client.use { exchange(http, trust, address, macName, it) }
            } finally {
                http.discard()
            }
        }

    private fun exchange(
        http: OkHttpClient, trust: FirstSeenTrustManager, address: String, macName: String, client: PairingClient,
    ): PairOutcome {
        try {
            val base = "https://$address"
            val encoder = Base64.getEncoder()
            val decoder = Base64.getDecoder()

            // 1. This device's first message, and the Mac's in return.
            val start = json.encodeToString(StartRequest(client.deviceName(), encoder.encodeToString(client.hello())))
            val begun = http.newCall(
                Request.Builder().url("$base/v1/pair/start").post(start.toRequestBody(jsonType)).build()
            ).execute().use { response ->
                val body = response.body.string()
                if (response.code == 403) return PairOutcome.NotOffering
                if (!response.isSuccessful) return PairOutcome.Unreachable
                json.decodeFromString<StartResponse>(body)
            }
            // The certificate this exchange is being made with. The client accepts no other from here on.
            val fingerprint = trust.fingerprint ?: return PairOutcome.Unreachable

            // 2. This device's proof that it knows the code, bound to that certificate.
            val confirmation = client.finish(decoder.decode(begun.message), fingerprint)
            val finish = json.encodeToString(FinishRequest(begun.session, encoder.encodeToString(confirmation)))
            val ended = http.newCall(
                Request.Builder().url("$base/v1/pair/finish").post(finish.toRequestBody(jsonType)).build()
            ).execute().use { response ->
                val body = response.body.string()
                when {
                    response.code == 409 -> return PairOutcome.TooManyDevices
                    response.code == 403 ->
                        return if (refusal(body).contains("closed")) PairOutcome.NotOffering else PairOutcome.WrongCode
                    !response.isSuccessful -> return PairOutcome.Unreachable
                }
                json.decodeFromString<FinishResponse>(body)
            }

            // 3. The Mac's proof that it knows the code too. Only now is anything it sent believed.
            val paired = client.accept(decoder.decode(ended.confirmation), decoder.decode(ended.grant))
            return PairOutcome.Paired(PairedMac(macName, fingerprint, address, paired.deviceId), paired.token)
        } catch (_: PairingException) {
            return PairOutcome.WrongCode
        } catch (_: IOException) {
            return PairOutcome.Unreachable
        } catch (_: IllegalArgumentException) {
            // An address that is no address, or an answer that is not what a Mac sends.
            return PairOutcome.Unreachable
        }
    }
}
