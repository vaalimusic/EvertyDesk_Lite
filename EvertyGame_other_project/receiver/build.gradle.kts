plugins {
    alias(libs.plugins.kotlin.jvm)
    application
}

kotlin {
    jvmToolchain(17)
}

application {
    mainClass = "com.everty.receiver.ReceiverAppKt"
}

dependencies {
    implementation(libs.javacv.platform)
    testImplementation(libs.junit)
}

tasks.test {
    useJUnit()
}
