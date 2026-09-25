// The UniFFI binding: `libleaf_ffi.so` for each ABI and the Kotlin generated
// from it, both written under build/rust/ by `cargo xtask android` and neither
// tracked. A host that links leaf-ffi into a larger Rust library of its own
// (one .so carrying several UniFFI components) supplies this module's
// `uniffi.leaf_ffi` package itself and leaves this module out.
plugins {
    alias(libs.plugins.android.library)
}

android {
    namespace = "org.diaryx.leaf.ffi"
    compileSdk = 37

    defaultConfig {
        // 29 is the floor for ELF thread-locals in bionic; below it the Zig
        // static library inside libleaf_ffi.so (twig) cannot load.
        minSdk = 29
    }

    sourceSets {
        getByName("main") {
            jniLibs.directories.add("build/rust/jniLibs")
            kotlin.directories.add("build/rust/kotlin")
        }
    }
}

dependencies {
    api(libs.jna) { artifact { type = "aar" } }
}

// A build without the binding fails at the first `uniffi.leaf_ffi` reference
// with a wall of unresolved names; say what is missing instead.
val checkBinding by tasks.registering {
    val binding = layout.projectDirectory.file("build/rust/kotlin/uniffi/leaf_ffi/leaf_ffi.kt")
    doLast {
        check(binding.asFile.isFile) {
            "The leaf-ffi binding has not been generated. Run `cargo xtask android` " +
                "(or `cargo xtask android --ffi-only`) from the leaf checkout first."
        }
    }
}
tasks.named("preBuild") { dependsOn(checkBinding) }
