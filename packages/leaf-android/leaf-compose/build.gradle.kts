// LeafEditor, the Compose editor over the binding. It compiles against
// leaf-ffi's `uniffi.leaf_ffi` without carrying it, so a host can bring that
// package from a library of its own; apps/leaf-android depends on both.
plugins {
    alias(libs.plugins.android.library)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "org.diaryx.leaf.compose"
    compileSdk = 37

    defaultConfig {
        minSdk = 29
    }

    buildFeatures {
        compose = true
    }
}

dependencies {
    compileOnly(project(":leaf-ffi"))
    implementation(platform(libs.compose.bom))
    api(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.material.icons)
    implementation(libs.compose.ui)
    testImplementation(libs.junit)
}
