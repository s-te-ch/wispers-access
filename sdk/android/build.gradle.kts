// The Wispers Access SDK for Android: the UniFFI-generated Kotlin API over
// the Rust library. Both the native libraries and the Kotlin source are
// produced by build-jnilibs.sh next to this file and are not checked in; run
// it once before building an app that depends on this module.

plugins {
    alias(libs.plugins.android.library)
}

android {
    namespace = "dev.wispers.access.sdk"
    compileSdk {
        version = release(36) {
            minorApiLevel = 1
        }
    }
    defaultConfig {
        minSdk = 28
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_11
        targetCompatibility = JavaVersion.VERSION_11
    }
}

dependencies {
    // The generated bindings call into the native library through JNA; the
    // AAR carries JNA's own native dispatch library per ABI.
    api("net.java.dev.jna:jna:${libs.versions.jna.get()}@aar")
    api(libs.kotlinx.coroutines.core)

    // Unit tests run on the host JVM against the macOS build of the library.
    testImplementation("net.java.dev.jna:jna:${libs.versions.jna.get()}")
    testImplementation(libs.junit)
}

// Where JNA finds libwispers_access_sdk.dylib for the host tests: the Rust
// workspace's release output, which build-jnilibs.sh leaves behind.
tasks.withType<Test>().configureEach {
    systemProperty("jna.library.path", rootProject.projectDir.resolve("../target/release").canonicalPath)
}
