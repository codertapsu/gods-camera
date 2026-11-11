package dev.godscamera.android

import java.security.MessageDigest

/**
 * The SHA-256 of a certificate in its DER form, as 64 lowercase hexadecimal
 * digits. This is how the Mac names its certificate, and what this device
 * remembers about the Mac after connecting.
 */
fun fingerprintOf(certificateDer: ByteArray): String =
    MessageDigest.getInstance("SHA-256").digest(certificateDer).joinToString("") { "%02x".format(it) }

/** Whether [text] has the form of a fingerprint. */
fun isFingerprint(text: String): Boolean = text.length == 64 && text.all { it in '0'..'9' || it in 'a'..'f' }

/** Compare two fingerprints without the time taken telling where they differ. */
fun sameFingerprint(a: String, b: String): Boolean =
    MessageDigest.isEqual(a.lowercase().toByteArray(Charsets.US_ASCII), b.lowercase().toByteArray(Charsets.US_ASCII))

/** Where a Mac listens for connected devices, unless told otherwise. */
const val DEFAULT_MAC_PORT = 47801

/**
 * Tidy an address a person typed into the form `host:port`, or null if it
 * cannot be one. The port may be left out, and a pasted `https://` is forgiven.
 */
fun parseMacAddress(typed: String): String? {
    var text = typed.trim().removePrefix("https://").removePrefix("http://").trimEnd('/')
    if (text.isEmpty() || text.any { it.isWhitespace() || it == '/' || it == '@' }) return null
    val host: String
    var port = DEFAULT_MAC_PORT
    if (text.startsWith("[")) {
        // An IPv6 address in brackets, perhaps followed by a port.
        val end = text.indexOf(']')
        if (end < 0) return null
        host = text.substring(0, end + 1)
        val rest = text.substring(end + 1)
        if (rest.isNotEmpty()) {
            if (!rest.startsWith(":")) return null
            port = rest.drop(1).toIntOrNull() ?: return null
        }
        if (host.length < 4 || host.drop(1).dropLast(1).any { !(it.isLetterOrDigit() || it == ':' || it == '.') }) return null
    } else {
        val colon = text.lastIndexOf(':')
        if (colon >= 0) {
            // More than one colon without brackets is an IPv6 address written bare.
            if (text.indexOf(':') != colon) return null
            port = text.substring(colon + 1).toIntOrNull() ?: return null
            text = text.substring(0, colon)
        }
        host = text
        if (host.isEmpty() || host.startsWith(".") || host.endsWith(".") || host.any { !(it.isLetterOrDigit() || it == '.' || it == '-') }) return null
    }
    if (port !in 1..65535) return null
    return "$host:$port"
}
