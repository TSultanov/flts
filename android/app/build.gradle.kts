plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

val repoRoot = rootProject.projectDir.parentFile

// The verifier's Kotlin half must match the crate the core was built with.
val rustlsVerifierVersion: String = run {
    val lines = File(repoRoot, "Cargo.lock").readLines()
    val at = lines.indexOfFirst { it.trim() == "name = \"rustls-platform-verifier-android\"" }
    check(at >= 0) { "rustls-platform-verifier-android not in Cargo.lock" }
    lines.drop(at + 1).first { it.trimStart().startsWith("version = ") }.substringAfter('"').substringBefore('"')
}

fun env(name: String): String? = System.getenv(name)?.takeIf { it.isNotEmpty() }

val abis = (env("FLTS_ANDROID_ABIS") ?: "arm64-v8a").split(",")

android {
    namespace = "com.TS.FLTS"
    compileSdk {
        version = release(37) {
            minorApiLevel = 1
        }
    }

    defaultConfig {
        // The Tauri app's id, so installing over it keeps its data.
        applicationId = "com.TS.FLTS"
        minSdk = 29
        targetSdk = 36
        // Above the Tauri build's 1000 (0.1.0); see docs/native-android.md.
        versionCode = 1100
        versionName = "0.1.0"
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
        // Only where the core is built: libraries bring their own .so for every ABI.
        ndk { abiFilters += abis }
    }

    signingConfigs {
        // Same key as site/build-android.sh, or the update over the Tauri app is refused.
        create("release") {
            storeFile = file(env("FLTS_KEYSTORE") ?: "${System.getProperty("user.home")}/.android/debug.keystore")
            keyAlias = env("FLTS_KEY_ALIAS") ?: "androiddebugkey"
            storePassword = env("FLTS_STORE_PASS") ?: "android"
            keyPassword = env("FLTS_KEY_PASS") ?: "android"
        }
    }

    buildTypes {
        debug {
            // Never the real app's data.
            applicationIdSuffix = ".dev"
        }
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            signingConfig = signingConfigs.getByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
    testOptions {
        unitTests.isIncludeAndroidResources = true
    }
}

kotlin {
    jvmToolchain(21)
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// The Rust core and its bindings, as native/scripts/build-core.sh does for Xcode.
// FLTS_SKIP_CORE=1 reuses what is already in jniLibs.
for ((variant, profile) in listOf("Debug" to "debug", "Release" to "release-ship")) {
    val buildCore = tasks.register<Exec>("buildCore$variant") {
        workingDir = repoRoot
        commandLine(listOf("android/scripts/build-core.sh", env("FLTS_CORE_PROFILE") ?: profile) + abis)
        onlyIf { env("FLTS_SKIP_CORE") == null }
    }
    tasks.matching { it.name == "merge${variant}JniLibFolders" || it.name == "compile${variant}Kotlin" }
        .configureEach { dependsOn(buildCore) }
}

// The core smoke test runs the bindings against the host's build of the core.
tasks.withType<Test>().configureEach {
    // One core per process.
    forkEvery = 1
    jvmArgs("--add-opens=java.base/jdk.internal.access=ALL-UNNAMED")
    env("FLTS_SMOKE_CONFIG_DIR")?.let { dir ->
        environment("FLTS_CONFIG_DIR", dir)
        systemProperty("jna.library.path", File(repoRoot, "target/debug").path)
        environment("FLTS_UITEST_EPUB", File(repoRoot, "native/scripts/fixtures/import.epub").path)
    }
}

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.11.01")
    implementation(composeBom)
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.1")
    implementation("androidx.lifecycle:lifecycle-process:2.11.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.11.0")

    // UniFFI's Kotlin bindings call the core through JNA.
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    implementation("org.rustls:rustls-platform-verifier:$rustlsVerifierVersion")

    // Pairing: draw this device's code, scan another's.
    implementation("com.google.zxing:core:3.5.4")
    implementation("androidx.camera:camera-camera2:1.7.0")
    implementation("androidx.camera:camera-lifecycle:1.7.0")
    implementation("androidx.camera:camera-view:1.7.0")

    debugImplementation("androidx.compose.ui:ui-tooling")

    testImplementation("junit:junit:4.13.2")
    testImplementation("org.jetbrains.kotlinx:kotlinx-coroutines-test:1.11.0")
    testImplementation("org.robolectric:robolectric:4.18.1")
    testImplementation("androidx.test:core:1.7.0")
    // The aar has no desktop natives.
    testImplementation("net.java.dev.jna:jna:5.19.1")
    testImplementation(composeBom)
    testImplementation("androidx.compose.ui:ui-test-junit4")

    androidTestImplementation(composeBom)
    androidTestImplementation("androidx.test.ext:junit:1.3.0")
    androidTestImplementation("androidx.test:runner:1.7.0")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    androidTestImplementation("androidx.test.espresso:espresso-core:3.7.0")
    debugImplementation("androidx.compose.ui:ui-test-manifest")
}
