import java.util.Properties

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

// Optional dev-server URL: set `lanline.devServerUrl=http://192.168.1.50:5173`
// in local.properties (or `-Planline.devServerUrl=...`) to load the live Vite
// server instead of the bundled assets.
val devServerUrl: String = run {
    val fromProp = (project.findProperty("lanline.devServerUrl") as String?)?.trim()
    if (!fromProp.isNullOrEmpty()) return@run fromProp
    val lp = rootProject.file("local.properties")
    if (lp.exists()) {
        Properties().apply { lp.inputStream().use { load(it) } }
            .getProperty("lanline.devServerUrl", "").trim()
    } else {
        ""
    }
}

android {
    namespace = "land.lanline"
    compileSdk = 35

    defaultConfig {
        applicationId = "land.lanline"
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
        buildConfigField("String", "DEV_SERVER_URL", "\"$devServerUrl\"")
    }

    buildFeatures {
        buildConfig = true
        viewBinding = false
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.13.1")
    implementation("androidx.appcompat:appcompat:1.7.0")
    implementation("androidx.activity:activity-ktx:1.9.3")
    implementation("androidx.webkit:webkit:1.12.1")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
}

// Copy the built web client (client/web/dist) into the APK assets before every
// build, so `npm run build` + `./gradlew assembleDebug` stay in sync.
val syncWebAssets by tasks.registering(Copy::class) {
    val dist = rootProject.file("../web/dist")
    from(dist)
    into(layout.projectDirectory.dir("src/main/assets/web"))
    doFirst {
        if (!dist.exists()) {
            logger.warn("client/web/dist not found — run `npm run build` in client/web. " +
                "Building with whatever is already in assets/web.")
        }
    }
}

tasks.named("preBuild") {
    dependsOn(syncWebAssets)
}
