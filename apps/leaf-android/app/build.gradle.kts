plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "org.diaryx.leaf.app"
    compileSdk = 37

    defaultConfig {
        // The Apple app's bundle id: they are the same app.
        applicationId = "org.diaryx.leaf"
        minSdk = 29
        targetSdk = 36
        versionCode = 1
        versionName = "0.4.6"
    }

    buildFeatures {
        compose = true
    }

    buildTypes {
        release {
            // Signed with the debug key, so `cargo xtask android --release`
            // installs: this app is not published, and a store build would
            // name its own signing config here.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    packaging {
        jniLibs {
            // UniFFI finds the library by name through JNA, which needs it
            // extracted to disk rather than mapped from the APK.
            useLegacyPackaging = true
        }
    }
}

dependencies {
    implementation("org.diaryx.leaf:leaf-ffi")
    implementation("org.diaryx.leaf:leaf-compose")
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.material3)
    implementation(libs.compose.ui)
    implementation(libs.compose.ui.tooling.preview)
    debugImplementation(libs.compose.ui.tooling)
    implementation(libs.activity.compose)
    implementation(libs.lifecycle.runtime.compose)
}
