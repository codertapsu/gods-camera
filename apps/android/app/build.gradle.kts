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
        // For BuildConfig.DEBUG, which gates the development-only token hand-over.
        buildConfig = true
    }

    testOptions {
        unitTests.all { test ->
            // The unit tests run the shared core on this computer's own Java,
            // and one of them pairs with a real hub started from the command-line tool.
            val target = rootProject.projectDir.resolve("../../core/target/release")
            test.systemProperty("jna.library.path", target.absolutePath)
            test.systemProperty("gc.tool", target.resolve("gc").absolutePath)
        }
    }

    lint {
        // The build does not pass with a new warning in it.
        warningsAsErrors = true
        // Newer versions of libraries are taken up deliberately, not when a check happens to run.
        disable += setOf("GradleDependency", "NewerVersionAvailable", "AndroidGradlePluginVersion")
        // The app targets Android 16 on purpose (docs/implementation-plan.md, section 4.2),
        // and the shared core is built for 64-bit ARM only.
        disable += setOf("OldTargetApi", "ChromeOsAbiSupport")
    }
}

// The shared core is built outside Gradle, by tools/build-core-android.sh. Its
// two products are not kept in git, so a fresh clone has neither: say so in
// plain words instead of failing later on a missing class or library.
val coreLibrary = layout.projectDirectory.file("src/main/jniLibs/arm64-v8a/libgc_mobile.so").asFile
val coreBindings = layout.projectDirectory.file("src/main/java/uniffi/gc_mobile/gc_mobile.kt").asFile
val checkCore = tasks.register("checkCore") {
    val library = coreLibrary
    val bindings = coreBindings
    doLast {
        if (!library.isFile || !bindings.isFile) {
            throw GradleException(
                "The shared core has not been built for Android. " +
                    "Run tools/build-core-android.sh from the repository's folder, then build again."
            )
        }
    }
}
tasks.named("preBuild") { dependsOn(checkCore) }

dependencies {
    implementation(platform("androidx.compose:compose-bom:2026.09.00"))
    implementation("androidx.compose.material3:material3")
    // The few stock icons the screens use. Material 3 no longer brings them along.
    implementation("androidx.compose.material:material-icons-core")
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
    // What the generated interface to the shared core calls it through.
    implementation("net.java.dev.jna:jna:5.19.1@aar")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.11.0")
    // The same library with the parts for desktop computers, so unit tests can load the core.
    testImplementation("net.java.dev.jna:jna:5.19.1")
}
