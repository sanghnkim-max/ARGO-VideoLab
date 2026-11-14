import java.security.KeyStore
import java.security.MessageDigest

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

// ── Release signing: ARGO's key ───────────────────────────────────────────────────────────────
//
// VideoLab ships as one of ARGO's companion apps: ARGO (Settings > Companion apps) installs it from
// its catalog only when it is signed exactly like ARGO, and each trusts the other by that same
// signature (`LabService` here, `LabLink` in ARGO). So a release is signed with ARGO's release
// key, read by the same names as ARGO's own build (ARGOv2 apps/argo-android/app/build.gradle.kts,
// "Release signing"), from the environment or the matching -P property:
//   ARGO_RELEASE_STORE_FILE      / argoReleaseStoreFile      the PKCS12 keystore (absolute, or relative to android/)
//   ARGO_RELEASE_STORE_PASSWORD  / argoReleaseStorePassword
//   ARGO_RELEASE_KEY_ALIAS       / argoReleaseKeyAlias
//   ARGO_RELEASE_KEY_PASSWORD    / argoReleaseKeyPassword
// Read here so a debug build needs none of them; a release refuses to package without them (below).
fun releaseValue(env: String, property: String): String? =
    (System.getenv(env) ?: findProperty(property) as String?)?.trim()?.ifEmpty { null }

val releaseStoreFile = releaseValue("ARGO_RELEASE_STORE_FILE", "argoReleaseStoreFile")
val releaseStorePassword = releaseValue("ARGO_RELEASE_STORE_PASSWORD", "argoReleaseStorePassword")
val releaseKeyAlias = releaseValue("ARGO_RELEASE_KEY_ALIAS", "argoReleaseKeyAlias")
val releaseKeyPassword = releaseValue("ARGO_RELEASE_KEY_PASSWORD", "argoReleaseKeyPassword")

/** ARGO's committed, public debug key (ARGOv2 `apps/argo-android/debug.keystore`, copied to `argo-debug.keystore`). */
val argoDebugCertSha256 = "4eecf98c331b09ebb54559b60ab544856e21c76f1239fdf9e3916bbf60f63c6a"

// Which of ARGO's keys a release is signed with: `release` (the default, above) or `debug`, ARGO's
// public debug key. ARGO installs and binds a companion only when it is signed like ARGO itself,
// and ARGO reaches its testers as debug builds for now (its Android bundle's VIA is `off`, which a
// release ARGO refuses to boot), so the catalog's companions are signed with the debug key until
// ARGO ships release builds: `ARGO_COMPANION_KEY=debug` or `-PargoCompanionKey=debug`.
val companionKey = releaseValue("ARGO_COMPANION_KEY", "argoCompanionKey") ?: "release"
require(companionKey == "release" || companionKey == "debug") {
    "videolab: ARGO_COMPANION_KEY / argoCompanionKey is `release` or `debug`, not `$companionKey`"
}
val signsWithArgoDebugKey = companionKey == "debug"

android {
    namespace = "dev.argo.videolab"
    compileSdk = 37

    defaultConfig {
        applicationId = "dev.argo.videolab"
        // Android 13: Vulkan 1.1 (FilmCraft's wgpu compositor), AImageReader + Codec2 MediaCodec
        // (hardware decoding), and the NDK media APIs the core links against.
        minSdk = 33
        targetSdk = 37
        versionCode = 2
        versionName = "0.1.1"
        ndk {
            // arm64 phones and the Apple-silicon emulator. Add x86_64 only when a matrix needs it.
            abiFilters += "arm64-v8a"
        }
    }

    signingConfigs {
        create("release") {
            if (signsWithArgoDebugKey) {
                storeFile = rootProject.file("argo-debug.keystore")
                storePassword = "android"
                keyAlias = "androiddebugkey"
                keyPassword = "android"
            } else {
                releaseStoreFile?.let { storeFile = rootProject.file(it) }
                storePassword = releaseStorePassword
                keyAlias = releaseKeyAlias
                keyPassword = releaseKeyPassword
            }
        }
    }

    buildTypes {
        release {
            signingConfig = signingConfigs.getByName("release")
            // No shrinking: the uniffi bindings reach the core through JNA by reflection.
            isMinifyEnabled = false
        }
    }

    buildFeatures {
        compose = true
        buildConfig = true
        aidl = true
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

// A release is signed with the ARGO key it asked for: checked when it is about to be signed or
// packaged (not at configuration, so debug builds need no secrets). With the release key it fails
// when a secret is missing and when the keystore holds ARGO's public debug key; with the debug key
// it fails unless the keystore is that key. Plain values captured, for the configuration cache.
run {
    val withDebugKey = signsWithArgoDebugKey
    val missing = if (withDebugKey) emptyList() else listOfNotNull(
        "ARGO_RELEASE_STORE_FILE".takeIf { releaseStoreFile == null },
        "ARGO_RELEASE_STORE_PASSWORD".takeIf { releaseStorePassword == null },
        "ARGO_RELEASE_KEY_ALIAS".takeIf { releaseKeyAlias == null },
        "ARGO_RELEASE_KEY_PASSWORD".takeIf { releaseKeyPassword == null },
    )
    val storePath = if (withDebugKey) rootProject.file("argo-debug.keystore").absolutePath else releaseStoreFile?.let { rootProject.file(it).absolutePath }
    val storePassword = if (withDebugKey) "android" else releaseStorePassword
    val alias = if (withDebugKey) "androiddebugkey" else releaseKeyAlias
    val debugDigest = argoDebugCertSha256
    val signingTasks = setOf("validateSigningRelease", "packageRelease", "signReleaseBundle", "packageReleaseBundle")
    tasks.matching { it.name in signingTasks }.configureEach {
        doFirst {
            if (missing.isNotEmpty()) {
                throw GradleException(
                    "videolab: a release is signed with ARGO's release key (it ships as an ARGO companion app), and " +
                        "${missing.joinToString()} ${if (missing.size == 1) "is" else "are"} not set (environment, or the " +
                        "matching -PargoRelease… property). Build the debug variant to test without it.",
                )
            }
            val store = KeyStore.getInstance(File(storePath!!), storePassword!!.toCharArray())
            val certificate = store.getCertificate(alias)
                ?: throw GradleException("videolab: the release keystore has no certificate under alias '$alias'")
            val digest = MessageDigest.getInstance("SHA-256").digest(certificate.encoded).joinToString("") { "%02x".format(it) }
            if (withDebugKey && digest != debugDigest) {
                throw GradleException("videolab: argo-debug.keystore is not ARGO's public debug key ($digest)")
            }
            if (!withDebugKey && digest == debugDigest) {
                throw GradleException(
                    "videolab: the release would be signed with ARGO's public debug key; use the release key, " +
                        "or ask for it with ARGO_COMPANION_KEY=debug.",
                )
            }
        }
    }
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
