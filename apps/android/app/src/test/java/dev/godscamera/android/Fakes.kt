package dev.godscamera.android

import android.content.SharedPreferences

/** Preferences kept in memory, for tests that run without a device. */
class FakePreferences : SharedPreferences {
    val values = mutableMapOf<String, Any?>()

    override fun getAll(): MutableMap<String, *> = values.toMutableMap()
    override fun getString(key: String, default: String?): String? = values[key] as? String ?: default
    @Suppress("UNCHECKED_CAST")
    override fun getStringSet(key: String, default: MutableSet<String>?): MutableSet<String>? = values[key] as? MutableSet<String> ?: default
    override fun getInt(key: String, default: Int): Int = values[key] as? Int ?: default
    override fun getLong(key: String, default: Long): Long = values[key] as? Long ?: default
    override fun getFloat(key: String, default: Float): Float = values[key] as? Float ?: default
    override fun getBoolean(key: String, default: Boolean): Boolean = values[key] as? Boolean ?: default
    override fun contains(key: String): Boolean = values.containsKey(key)
    override fun registerOnSharedPreferenceChangeListener(listener: SharedPreferences.OnSharedPreferenceChangeListener?) {}
    override fun unregisterOnSharedPreferenceChangeListener(listener: SharedPreferences.OnSharedPreferenceChangeListener?) {}

    override fun edit(): SharedPreferences.Editor = object : SharedPreferences.Editor {
        private val pending = mutableMapOf<String, Any?>()
        private val removed = mutableSetOf<String>()
        private var clearing = false

        private fun put(key: String, value: Any?) = apply { pending[key] = value }
        override fun putString(key: String, value: String?) = put(key, value)
        override fun putStringSet(key: String, value: MutableSet<String>?) = put(key, value)
        override fun putInt(key: String, value: Int) = put(key, value)
        override fun putLong(key: String, value: Long) = put(key, value)
        override fun putFloat(key: String, value: Float) = put(key, value)
        override fun putBoolean(key: String, value: Boolean) = put(key, value)
        override fun remove(key: String) = apply { removed += key }
        override fun clear() = apply { clearing = true }
        override fun commit(): Boolean {
            if (clearing) values.clear()
            removed.forEach(values::remove)
            values.putAll(pending)
            return true
        }
        override fun apply() {
            commit()
        }
    }
}

/** Seals by turning the text round, which is enough to see that the plain text is not what is stored. */
class FakeBox : SecretBox {
    override fun seal(secret: String): String = "sealed:" + secret.reversed()
    override fun open(sealed: String): String? = sealed.removePrefix("sealed:").takeIf { it != sealed }?.reversed()
}
