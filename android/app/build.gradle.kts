// VideoLab for Android: a Compose shell over FilmCraft's engine.
//
// The Rust side (../../crates) builds with cargo-ndk into this module's jniLibs:
//   * libvideolab_ffi.so: the core (videolab-ffi, uniffi). Kotlin bindings are generated from
//     the debug .so by uniffi-bindgen into build/generated/uniffi.
//   * libfilmcraft_cli.so: FilmCraft's own `filmcraft-cli`, unmodified, built from the pinned rev
//     (tools/build-filmcraft-cli.sh). Named like a library so the installer extracts it to
//     nativeLibraryDir with the exec bit: the app's console and agent run it with
//     `--bridge` to the core's control channel, and `adb shell` can too. Android only allows
//     exec of files there.
//   * libvideolab_cli.so: the `videolab` executable (registry call/help/script) for adb shell.
//
// No `org.jetbrains.kotlin.android` plugin: AGP 9 compiles Kotlin built in.

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.serialization)
    alias(libs.plugins.kotlin.compose)
}

android {
    namespace = "dev.argo.videolab"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.argo.videolab"
        // Android 13: Vulkan 1.1 (FilmCraft's wgpu compositor), AImageReader + Codec2 MediaCodec
        // (hardware decoding), and the NDK media APIs the core links against.
        minSdk = 33
        targetSdk = 37
        versionCode = 1
        versionName = "0.1.0"
        ndk {
            // arm64 phones and the Apple-silicon emulator. Add x86_64 only when a matrix needs it.
            abiFilters += "arm64-v8a"
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

    packaging {
        jniLibs {
            // Extract .so files to nativeLibraryDir: the CLIs must exist there as real files to
            // be executed (by the app and from `adb shell`).
            useLegacyPackaging = true
        }
    }

    // A plain File: AGP 9's SourceSet API refuses a Provider here. preBuild depends on
    // uniffiBindgen, so the directory is filled before anything reads it.
    sourceSets["main"].kotlin.directories.add(layout.buildDirectory.dir("generated/uniffi").get().asFile.path)
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.kotlinx.serialization.json)
    implementation(libs.kotlinx.coroutines.android)
    implementation("${libs.jna.get()}@aar")
    implementation(platform(libs.compose.bom))
    implementation(libs.compose.ui)
    implementation(libs.compose.foundation)
    implementation(libs.compose.material3)
    implementation(libs.compose.ui.tooling.preview)
    implementation(libs.androidx.activity.compose)
    implementation(libs.androidx.lifecycle.runtime.compose)
    implementation(libs.androidx.lifecycle.viewmodel.compose)
    implementation(libs.androidx.window)
    implementation(libs.androidx.window.core)
    implementation(libs.okhttp)
    implementation(libs.anthropic.java)
    debugImplementation(libs.compose.ui.tooling)
    testImplementation(libs.junit)
}

// ── Rust ─────────────────────────────────────────────────────────────────────────────────────

val repoRoot: File = rootProject.projectDir.parentFile
val abi = "arm64-v8a"
val ndkTarget = "aarch64-linux-android"
// At or below minSdk: a .so built for an older platform loads on newer ones.
val ndkPlatform = "33"
val ffiLib = "libvideolab_ffi.so"
val uniffiOutDir = layout.buildDirectory.dir("generated/uniffi")

// Cargo decides what is stale; Gradle always asks it (no inputs over the Rust sources).
fun cargoNdkTask(buildType: String, release: Boolean) =
    tasks.register<Exec>("cargoNdk${buildType.replaceFirstChar { it.uppercase() }}") {
        description = "Builds the $buildType Rust core, the videolab CLI and FilmCraft's filmcraft-cli with cargo-ndk."
        val jniLibs = layout.projectDirectory.dir("src/$buildType/jniLibs")
        val profileDir = if (release) "release" else "debug"
        workingDir = repoRoot
        commandLine(
            listOf("cargo", "ndk", "-t", abi, "--platform", ndkPlatform, "-o", jniLibs.asFile.path, "build") +
                (if (release) listOf("--release") else emptyList()) +
                listOf("-p", "videolab-ffi", "-p", "videolab-cli"),
        )
        doLast {
            // cargo-ndk copies cdylibs only; the CLI executables ride along as "libraries".
            val cli = repoRoot.resolve("target/$ndkTarget/$profileDir/videolab")
            val dest = jniLibs.dir(abi).file("libvideolab_cli.so").asFile
            dest.parentFile.mkdirs()
            cli.copyTo(dest, overwrite = true)
            // FilmCraft's own CLI, unmodified, from cargo's checkout of the pinned rev.
            project.providers.exec {
                workingDir = repoRoot
                commandLine(
                    listOf("tools/build-filmcraft-cli.sh", jniLibs.dir(abi).file("libfilmcraft_cli.so").asFile.path) +
                        (if (release) listOf("--release") else emptyList()),
                )
                environment("NDK_PLATFORM", ndkPlatform)
            }.result.get()
        }
        outputs.upToDateWhen { false }
    }

val cargoNdkDebug = cargoNdkTask("debug", release = false)
val cargoNdkRelease = cargoNdkTask("release", release = true)

val uniffiBindgen by tasks.registering(Exec::class) {
    description = "Generates the Kotlin bindings from the unstripped debug .so."
    dependsOn(cargoNdkDebug)
    workingDir = repoRoot
    val so = repoRoot.resolve("target/$ndkTarget/debug/$ffiLib")
    doFirst { uniffiOutDir.get().asFile.mkdirs() }
    commandLine(
        "cargo", "run", "--locked", "-p", "uniffi-bindgen", "--",
        "generate", "--library", so.path,
        "--language", "kotlin",
        "--no-format",
        "--out-dir", uniffiOutDir.get().asFile.path,
    )
    outputs.dir(uniffiOutDir)
    outputs.upToDateWhen { false }
}

tasks.named("preBuild") { dependsOn(uniffiBindgen) }
tasks.configureEach {
    when (name) {
        "preDebugBuild" -> dependsOn(cargoNdkDebug)
        "preReleaseBuild" -> dependsOn(cargoNdkRelease)
    }
}
