import java.util.Properties

val releaseKeystorePropertiesFile = rootProject.file("keystore.properties")
val releaseKeystoreProperties = Properties().apply {
    if (releaseKeystorePropertiesFile.exists()) {
        releaseKeystorePropertiesFile.inputStream().use { load(it) }
    }
}
val hasReleaseKeystore = releaseKeystorePropertiesFile.exists()

fun Properties.signingProperty(name: String): String =
    getProperty(name)
        ?: getProperty("\uFEFF$name")
        ?: error("Missing release signing property: $name")

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
        versionCode = 2
        versionName = "0.2"
        ndk {
            // Архитектуры устройств. arm64 — основная для современных телефонов.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a")
        }
    }

    // Нативные .so кладутся в src/main/jniLibs/<abi>/libevertydesk_core.so
    // (собираются через cargo-ndk — см. android/README.md)
    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")

    if (hasReleaseKeystore) {
        signingConfigs {
            create("release") {
                storeFile = rootProject.file(releaseKeystoreProperties.signingProperty("storeFile"))
                storePassword = releaseKeystoreProperties.signingProperty("storePassword")
                keyAlias = releaseKeystoreProperties.signingProperty("keyAlias")
                keyPassword = releaseKeystoreProperties.signingProperty("keyPassword")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            if (hasReleaseKeystore) {
                signingConfig = signingConfigs.getByName("release")
            }
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
    // Шифрование SharedPreferences через Android Keystore (аппаратный AES-256-GCM).
    // Пароли подключений хранятся в EncryptedSharedPreferences — не читаются
    // другими приложениями даже на устройствах с root.
    implementation("androidx.security:security-crypto:1.0.0")
}
