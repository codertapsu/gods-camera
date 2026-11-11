// Versions are set once here. Kotlin itself comes with the Android plugin;
// the two Kotlin plugins below add Compose and JSON support.
plugins {
    id("com.android.application") version "9.5.1" apply false
    id("org.jetbrains.kotlin.plugin.compose") version "2.4.21" apply false
    id("org.jetbrains.kotlin.plugin.serialization") version "2.4.21" apply false
}
