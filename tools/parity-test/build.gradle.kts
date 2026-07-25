plugins {
    kotlin("jvm") version "2.0.21"
}

repositories {
    mavenCentral()
}

dependencies {
    implementation("org.signal.forks:noise-java:0.1.1")
    implementation("com.fasterxml.jackson.module:jackson-module-kotlin:2.18.0")
    testImplementation(kotlin("test"))
}

kotlin {
    jvmToolchain(17)
}

tasks.test {
    useJUnitPlatform()
}
