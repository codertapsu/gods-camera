package dev.godscamera.android

import android.content.SharedPreferences
import android.security.keystore.KeyGenParameterSpec
import android.security.keystore.KeyProperties
import androidx.core.content.edit
import java.security.GeneralSecurityException
import java.security.KeyStore
import java.util.Base64
import javax.crypto.Cipher
import javax.crypto.KeyGenerator
import javax.crypto.SecretKey
import javax.crypto.spec.GCMParameterSpec

/** Seals a secret so that only this app on this device can open it again. */
interface SecretBox {
    fun seal(secret: String): String

    /** The secret, or null if it cannot be opened: the key is gone or the text was changed. */
    fun open(sealed: String): String?
}

/**
 * Encrypts with a key that lives in the Android Keystore. The key never
 * leaves the device's secure hardware and cannot be copied out with the app's
 * files, so the sealed text is useless anywhere else.
 */
class KeystoreBox(private val alias: String = "gc.mac.token") : SecretBox {
    private fun key(): SecretKey {
        val store = KeyStore.getInstance("AndroidKeyStore").apply { load(null) }
        (store.getKey(alias, null) as? SecretKey)?.let { return it }
        val generator = KeyGenerator.getInstance(KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore")
        generator.init(
            KeyGenParameterSpec.Builder(alias, KeyProperties.PURPOSE_ENCRYPT or KeyProperties.PURPOSE_DECRYPT)
                .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                .setKeySize(256)
                .build()
        )
        return generator.generateKey()
    }

    override fun seal(secret: String): String {
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.ENCRYPT_MODE, key())
        val sealed = cipher.doFinal(secret.toByteArray(Charsets.UTF_8))
        // The keystore chooses a fresh 12-byte nonce for every seal; it is kept in front.
        return Base64.getEncoder().encodeToString(cipher.iv + sealed)
    }

    override fun open(sealed: String): String? = try {
        val bytes = Base64.getDecoder().decode(sealed)
        val cipher = Cipher.getInstance("AES/GCM/NoPadding")
        cipher.init(Cipher.DECRYPT_MODE, key(), GCMParameterSpec(128, bytes, 0, 12))
        String(cipher.doFinal(bytes, 12, bytes.size - 12), Charsets.UTF_8)
    } catch (_: GeneralSecurityException) {
        null
    } catch (_: IllegalArgumentException) {
        null
    }

    /** Throw the key away. Whatever it sealed can never be opened again. */
    fun destroy() {
        runCatching { KeyStore.getInstance("AndroidKeyStore").apply { load(null) }.deleteEntry(alias) }
    }
}

/**
 * What this device remembers about the Mac it is connected to.
 *
 * The Mac's name, certificate fingerprint and address are not secrets and are
 * kept as they are. The token is what lets this device send to the Mac, so it
 * is kept sealed by [box].
 */
class PairingStore(private val preferences: SharedPreferences, private val box: SecretBox) {
    /** The Mac and the token for it, or null when there is none or the token can no longer be opened. */
    fun load(): Pair<PairedMac, String>? {
        val name = preferences.getString("name", null) ?: return null
        val fingerprint = preferences.getString("fingerprint", null)?.takeIf(::isFingerprint) ?: return null
        val address = preferences.getString("address", null) ?: return null
        val deviceId = preferences.getString("device_id", null) ?: return null
        val token = preferences.getString("token_sealed", null)?.let(box::open) ?: return null
        return PairedMac(name, fingerprint, address, deviceId) to token
    }

    fun save(mac: PairedMac, token: String) {
        preferences.edit {
            putString("name", mac.name)
            putString("fingerprint", mac.fingerprint)
            putString("address", mac.address)
            putString("device_id", mac.deviceId)
            putString("token_sealed", box.seal(token))
        }
    }

    /** The Mac was found at a new place on the network. */
    fun updateAddress(address: String) {
        if (preferences.contains("name")) preferences.edit { putString("address", address) }
    }

    fun clear() {
        preferences.edit { clear() }
    }
}
