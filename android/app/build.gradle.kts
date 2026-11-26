import java.io.File

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "cpasfait.app"
    compileSdk = 37

    defaultConfig {
        applicationId = "cpasfait.app"
        minSdk = 26
        targetSdk = 37
        versionCode = 1
        versionName = "0.1"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Until there is a release key: installable builds signed with the debug key.
            signingConfig = signingConfigs.getByName("debug")
        }
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        resources.excludes += setOf("META-INF/versions/9/OSGI-INF/MANIFEST.MF", "META-INF/DEPENDENCIES")
    }

    sourceSets.getByName("main") {
        jniLibs.directories.add(layout.buildDirectory.dir("rust/jniLibs").get().asFile.path)
        kotlin.directories.add(layout.buildDirectory.dir("rust/kotlin").get().asFile.path)
    }
}

// ------------------------------------------------------------------ the Rust core

// The cargo workspace (this directory's parent).
val workspace: File = rootDir.parentFile
val cargoBin = File(System.getProperty("user.home"), ".cargo/bin")
val jniOut = layout.buildDirectory.dir("rust/jniLibs")
val kotlinOut = layout.buildDirectory.dir("rust/kotlin")

// Debug builds of the app still get an optimized core (much faster, cached by cargo).
val cargoNdk by tasks.registering(Exec::class) {
    description = "Builds libcpasfait_ffi.so for the phone and the emulator"
    workingDir = workspace
    environment("PATH", "${cargoBin.path}:${System.getenv("PATH")}")
    val sdk = androidComponents.sdkComponents.sdkDirectory
    doFirst { environment("ANDROID_HOME", sdk.get().asFile.path) }
    commandLine(
        "cargo", "ndk", "-t", "arm64-v8a", "-t", "x86_64", "-o", jniOut.get().asFile.path,
        "build", "--release", "-p", "cpasfait-ffi",
    )
    // cargo knows what changed.
    outputs.upToDateWhen { false }
}

val uniffiBindings by tasks.registering(Exec::class) {
    description = "Generates the Kotlin bindings of the core"
    dependsOn(cargoNdk)
    workingDir = workspace
    environment("PATH", "${cargoBin.path}:${System.getenv("PATH")}")
    commandLine(
        "cargo", "run", "-q", "-p", "cpasfait-ffi", "--features", "cli", "--bin", "uniffi-bindgen", "--",
        "generate", "--library", jniOut.get().file("x86_64/libcpasfait_ffi.so").asFile.path,
        "--language", "kotlin", "--no-format", "--out-dir", kotlinOut.get().asFile.path,
    )
    outputs.upToDateWhen { false }
}

tasks.named("preBuild") { dependsOn(uniffiBindings) }

dependencies {
    val composeBom = platform("androidx.compose:compose-bom:2026.09.00")
    implementation(composeBom)
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.material:material-icons-extended")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    debugImplementation("androidx.compose.ui:ui-tooling")
    implementation("androidx.activity:activity-compose:1.13.0")
    implementation("androidx.core:core-ktx:1.19.1")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.11.1")
    implementation("androidx.work:work-runtime-ktx:2.12.0")
    // The bindings load the core through JNA.
    implementation("net.java.dev.jna:jna:5.19.1@aar")
    // SFTP (the rsync: remotes).
    implementation("com.hierynomus:sshj:0.41.2")
    implementation("org.bouncycastle:bcprov-jdk18on:1.84")
    implementation("org.bouncycastle:bcpkix-jdk18on:1.84")
}
