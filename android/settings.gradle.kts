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
        // Kotlin half of the Rust TLS verifier (rustls-platform-verifier).
        maven {
            url = uri("https://github.com/rustls/rustls-platform-verifier/raw/maven-archive/android-release-support/maven/")
            content { includeGroup("org.rustls") }
        }
    }
}

rootProject.name = "FLTS"
include(":app")
