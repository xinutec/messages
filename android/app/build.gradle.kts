plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
}

android {
    namespace = "org.xinutec.messages"
    compileSdk = 36
    // The build-tools version the read-only nix SDK provides.
    buildToolsVersion = "36.0.0"

    defaultConfig {
        applicationId = "org.xinutec.messages"
        // minSdk 26: the system WebView is Chromium.
        minSdk = 26
        targetSdk = 36
        versionCode = 1
        versionName = "0.1"
    }

    buildTypes {
        // Sideloaded: no shrinking, debug-signed.
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// A clear message when the shell is missing. Resolved against rootDir, as
// settings.gradle.kts includes it.
require(rootDir.resolve("../../ui-harness/android").isDirectory) {
    "ui-harness must be checked out beside this repo (~/Code/ui-harness)"
}

dependencies {
    // The shared WebView shell (ui-harness/android), a project by path via
    // settings.gradle.kts; it brings androidx.activity.
    implementation("org.xinutec:shell")
    // core-ktx for the prefs and insets KTX.
    implementation(libs.androidx.core.ktx)
}
