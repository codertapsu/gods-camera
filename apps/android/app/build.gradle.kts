plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
    id("org.jetbrains.kotlin.plugin.serialization")
}

android {
    namespace = "dev.godscamera.android"
    // The libraries are built against API 37. Building against it does not
    // change where the app runs: that is minSdk and targetSdk below.
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.godscamera.android"
        // Android 16 is the app's floor. See docs/implementation-plan.md, section 4.2.
        minSdk = 36
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"
        // The shared core is built for 64-bit ARM only.
        ndk { abiFilters += "arm64-v8a" }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    buildFeatures {
        compose = true
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.11.0")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")
    implementation("org.jetbrains.kotlinx:kotlinx-serialization-json:1.12.0")
    implementation("com.squareup.okhttp3:okhttp:5.5.1")
    // The document scanner: edge detection and clean-up, run by Google Play services.
    implementation("com.google.android.gms:play-services-mlkit-document-scanner:16.0.0")
}
