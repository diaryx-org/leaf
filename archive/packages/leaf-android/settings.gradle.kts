// leaf's Android library, as its own Gradle build: the UniFFI binding and the
// Compose editor over it. apps/leaf-android includes this build and depends on
// its modules by coordinates (`org.diaryx.leaf:leaf-compose`), which is how a
// host outside this repository will depend on them too.
pluginManagement {
    repositories {
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

dependencyResolutionManagement {
    repositoriesMode.set(RepositoriesMode.FAIL_ON_PROJECT_REPOS)
    repositories {
        google()
        mavenCentral()
    }
}

rootProject.name = "leaf-android"
include(":leaf-ffi", ":leaf-compose")
