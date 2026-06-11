plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "ru.everty.desklite"
    compileSdk = 34

    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "ru.everty.desklite"
        minSdk = 24          // Android 7.0 — нужен для современного MediaCodec/NDK
        targetSdk = 34
        versionCode = 1
        versionName = "0.1"
        ndk {
            // Архитектуры устройств. arm64 — основная для современных телефонов.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }
    }

    // Нативные .so кладутся в src/main/jniLibs/<abi>/libevertydesk_core.so
    // (собираются через cargo-ndk — см. android/README.md)
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")


    buildTypes {
        release {
            isMinifyEnabled = false
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
}
