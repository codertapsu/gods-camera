package dev.godscamera.android

import android.annotation.SuppressLint
import okhttp3.OkHttpClient
import java.security.MessageDigest
import java.security.cert.CertificateException
import java.security.cert.X509Certificate
import java.util.concurrent.TimeUnit
import javax.net.ssl.SSLContext
import javax.net.ssl.X509TrustManager

/**
 * Trusts one certificate only: the Mac's, as it was seen when this device was
 * connected with the code. The Mac makes its own certificate, so no authority
 * vouches for it; what vouches for it is that the person typed the code the
 * Mac showed. Any other certificate, valid or not, is turned away.
 */
@SuppressLint("CustomX509TrustManager") // Deliberate: a self-made certificate, pinned exactly. See above.
class PinnedTrustManager(private val fingerprint: String) : X509TrustManager {
    init {
        require(isFingerprint(fingerprint.lowercase())) { "a fingerprint is 64 hexadecimal digits" }
    }

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?): Unit =
        throw CertificateException("this app accepts no client certificates")

    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull() ?: throw CertificateException("the other side showed no certificate")
        if (!sameFingerprint(fingerprintOf(leaf.encoded), fingerprint)) {
            throw CertificateException("this is not the Mac this device is connected to")
        }
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

/**
 * Takes the first certificate it is shown, remembers it, and from then on
 * accepts no other. Used only for the exchange of the six-digit code, where
 * nothing is known about the Mac yet.
 *
 * Taking a certificate unseen is safe here and nowhere else: the exchange
 * itself proves, or fails to prove, that the certificate of this connection
 * belongs to the Mac that shows the code. Nothing received over such a
 * connection is believed until the exchange has succeeded, and because the
 * certificate cannot change in between, every message of one exchange comes
 * from the holder of the same certificate. One of these serves one attempt.
 */
@SuppressLint("CustomX509TrustManager") // Deliberate. See above.
class FirstSeenTrustManager : X509TrustManager {
    private var seen: ByteArray? = null

    /** The fingerprint of the certificate the connection was made with, or null before there was one. */
    val fingerprint: String?
        @Synchronized get() = seen?.let(::fingerprintOf)

    override fun checkClientTrusted(chain: Array<out X509Certificate>?, authType: String?): Unit =
        throw CertificateException("this app accepts no client certificates")

    @Synchronized
    override fun checkServerTrusted(chain: Array<out X509Certificate>?, authType: String?) {
        val leaf = chain?.firstOrNull()?.encoded ?: throw CertificateException("the other side showed no certificate")
        val first = seen
        if (first == null) {
            seen = leaf
        } else if (!MessageDigest.isEqual(first, leaf)) {
            throw CertificateException("the certificate changed in the middle of connecting")
        }
    }

    override fun getAcceptedIssuers(): Array<X509Certificate> = emptyArray()
}

/** The HTTP clients this app uses, one for each kind of trust. */
object HubHttp {
    private fun base(): OkHttpClient.Builder =
        OkHttpClient.Builder()
            .connectTimeout(4, TimeUnit.SECONDS)
            .readTimeout(60, TimeUnit.SECONDS)
            .writeTimeout(60, TimeUnit.SECONDS)

    private fun OkHttpClient.Builder.trusting(manager: X509TrustManager): OkHttpClient.Builder {
        val context = SSLContext.getInstance("TLS")
        context.init(null, arrayOf(manager), null)
        return sslSocketFactory(context.socketFactory, manager)
            // The Mac's certificate names no address: the Mac may be anywhere
            // on the network. Who it is, is settled by the certificate alone.
            .hostnameVerifier { _, _ -> true }
    }

    /** The client for the Mac in use, kept so that its connections are used again. */
    private var kept: Pair<String, OkHttpClient>? = null

    /** For a Mac this device is connected to: only its own certificate is accepted. */
    @Synchronized
    fun pinned(fingerprint: String): OkHttpClient {
        val key = fingerprint.lowercase()
        kept?.let { (pinnedTo, client) -> if (pinnedTo == key) return client }
        return base().trusting(PinnedTrustManager(key)).build().also { kept = key to it }
    }

    /**
     * For one exchange of the code, and nothing else. The trust manager that
     * comes with the client says which certificate the exchange was made with.
     */
    fun unproven(): Pair<OkHttpClient, FirstSeenTrustManager> {
        val trust = FirstSeenTrustManager()
        return base().trusting(trust).build() to trust
    }

    /** For the debugging connection, where the hub is this device's own localhost. */
    fun plain(): OkHttpClient = base().build()
}
