// The app module. Two Rust steps run before Kotlin compiles: `cargo ndk` builds
// `libligature_android.so` for each ABI into app/build/rustJniLibs, and
// `uniffi-bindgen` generates the Kotlin bindings from the arm64 library into
// app/build/generated/uniffi. Both are plain Exec tasks so nothing but cargo,
// cargo-ndk and the NDK is needed (milestone 2, LP-78).

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

val cargo: String = (project.findProperty("rust.cargo") as String?) ?: "cargo"
val rustAbis = listOf("arm64-v8a", "x86_64")
val rustJniLibs = layout.buildDirectory.dir("rustJniLibs")
val uniffiOut = layout.buildDirectory.dir("generated/uniffi")
val cargoManifest = rootProject.file("../Cargo.toml")

android {
    namespace = "org.ligatureprotocol.android"
    compileSdk = 36
    ndkVersion = "27.2.12479018"

    defaultConfig {
        applicationId = "org.ligatureprotocol.android"
        minSdk = 29
        targetSdk = 35
        versionCode = 1
        versionName = "0.0.1"
        ndk {
            abiFilters += rustAbis
        }
    }

    buildTypes {
        release {
            // Unsigned: sign afterwards with apksigner and either a debug key
            // or the ROM's platform key (README.md).
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets {
        getByName("main") {
            jniLibs.srcDir(rustJniLibs)
            kotlin.srcDir(uniffiOut)
        }
    }

    packaging {
        jniLibs {
            // One copy of the Rust library per ABI, uncompressed for direct mapping.
            useLegacyPackaging = false
        }
    }
}

kotlin {
    // Target 17 bytecode from whatever JDK runs Gradle (17 or newer), no toolchain download.
    compilerOptions {
        jvmTarget.set(org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17)
    }
}

// Builds the Rust library for every ABI. Release profile always: a debug
// build of the QUIC and crypto stack is slow on a phone and three times the size.
val cargoNdk by tasks.registering(Exec::class) {
    group = "rust"
    description = "Cross-builds libligature_android.so with cargo ndk"
    workingDir = projectDir
    inputs.files(cargoManifest, rootProject.file("../Cargo.lock"))
    inputs.dir(rootProject.file("../crates"))
    outputs.dir(rustJniLibs)
    doFirst {
        environment("ANDROID_NDK_HOME", android.ndkDirectory.absolutePath)
    }
    commandLine(
        listOf(cargo, "ndk", "--manifest-path", cargoManifest.absolutePath) +
            rustAbis.flatMap { listOf("-t", it) } +
            listOf("-o", rustJniLibs.get().asFile.absolutePath, "build", "-p", "ligature-android", "--release"),
    )
}

// Generates the Kotlin bindings from the compiled arm64 library (uniffi reads
// the metadata from the ELF file; it does not load it).
val uniffiBindings by tasks.registering(Exec::class) {
    group = "rust"
    description = "Generates the Kotlin bindings with uniffi-bindgen"
    dependsOn(cargoNdk)
    workingDir = projectDir
    inputs.dir(rustJniLibs)
    outputs.dir(uniffiOut)
    commandLine(
        cargo, "run", "--manifest-path", cargoManifest.absolutePath,
        "-p", "ligature-android", "--features", "bindgen", "--bin", "uniffi-bindgen", "--",
        "generate", "--library", rustJniLibs.get().file("arm64-v8a/libligature_android.so").asFile.absolutePath,
        "--language", "kotlin", "--no-format", "--out-dir", uniffiOut.get().asFile.absolutePath,
    )
}

tasks.named("preBuild") {
    dependsOn(uniffiBindings)
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.service)
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.kotlinx.coroutines.android)
    implementation("${libs.jna.get()}@aar")
    implementation(libs.zxing.core)
}
