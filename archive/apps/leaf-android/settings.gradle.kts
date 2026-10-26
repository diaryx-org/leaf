// Leaf for Android: the app, over the library build in packages/leaf-android.
// That build is *included*, so `org.diaryx.leaf:leaf-compose` below resolves to
// its source rather than to a published artifact — one version catalog, read
// from there, serves both.
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
    versionCatalogs {
        create("libs") {
            from(files("../../packages/leaf-android/gradle/libs.versions.toml"))
        }
    }
}

rootProject.name = "Leaf"
includeBuild("../../packages/leaf-android")
include(":app")
