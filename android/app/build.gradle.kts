import java.util.Properties

plugins {
    alias(libs.plugins.android.application)
    alias(libs.plugins.kotlin.android)
    alias(libs.plugins.kotlin.compose)
}

// Release signing reads keystore.properties (gitignored) when present:
//   storeFile=/abs/path/vortex.keystore
//   storePassword=…
//   keyAlias=…
//   keyPassword=…
// Without it the release build is simply unsigned — CI/dev machines
// don't need the keystore to verify the build compiles.
val keystoreProps = Properties().apply {
    val f = rootProject.file("keystore.properties")
    if (f.isFile) f.inputStream().use { load(it) }
}

android {
    namespace = "com.vortex.a3"
    compileSdk = 36

    defaultConfig {
        // Public identity — PERMANENT once shipped (a different id is a
        // different app to Android: no update path, pairing lost). GitHub-based
        // per the F-Droid convention since we own no domain; the `namespace`
        // above (internal code packages) intentionally keeps the old name.
        applicationId = "io.github.zoir_dev.vortex"
        minSdk = 29
        targetSdk = 36
        versionCode = 4
        versionName = "1.0.0-beta.4"
    }

    signingConfigs {
        if (keystoreProps.getProperty("storeFile") != null) {
            create("release") {
                storeFile = file(keystoreProps.getProperty("storeFile"))
                storePassword = keystoreProps.getProperty("storePassword")
                keyAlias = keystoreProps.getProperty("keyAlias")
                keyPassword = keystoreProps.getProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            // NOTE: the minified APK still needs a full on-device smoke
            // test (pairing, reconnect, call mirror, SMS) before any
            // release is trusted — proguard-rules.pro keeps the
            // reflection-heavy crypto, but only a live run proves it.
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
            signingConfig = signingConfigs.findByName("release")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    buildFeatures {
        compose = true
        buildConfig = true
    }

    testOptions {
        unitTests.isReturnDefaultValues = true
    }
}

dependencies {
    implementation(libs.androidx.core.ktx)
    implementation(libs.androidx.lifecycle.runtime.ktx)
    implementation(libs.androidx.activity.compose)
    implementation(platform(libs.androidx.compose.bom))
    implementation(libs.androidx.compose.ui)
    implementation(libs.androidx.compose.ui.tooling.preview)
    implementation(libs.androidx.compose.material3)
    implementation(libs.androidx.compose.material.icons.extended)
    implementation(libs.noise.java)
    implementation(libs.androidx.security.crypto)

    testImplementation(libs.jackson.databind)
    testImplementation(libs.junit.jupiter)
    testImplementation(libs.mockk)
    testImplementation(kotlin("test"))
    testImplementation(kotlin("test-junit5"))
    // org.json is part of Android's framework jar but stubbed in
    // local unit tests (`returnDefaultValues = true`). Bring the
    // reference implementation in for tests so AppState's JSON
    // parser actually runs against real `JSONObject` semantics.
    testImplementation("org.json:json:20240303")
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.withType<Test> {
    useJUnitPlatform()
}
