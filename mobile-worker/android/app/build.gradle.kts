import com.google.protobuf.gradle.id

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("com.google.protobuf")
}

android {
    namespace = "org.aiupcycler.worker"
    compileSdk = 34

    defaultConfig {
        applicationId = "org.aiupcycler.worker"
        minSdk = 26
        targetSdk = 34
        versionCode = 1
        versionName = "0.1.0"
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    // grpc-netty pulls extra META-INF metadata that collides when packaged into
    // a single APK; keep the first copy of the netty version manifest and drop
    // the human-readable licence/notice files.
    packaging {
        resources {
            pickFirsts += listOf(
                "META-INF/io.netty.versions.properties",
                "META-INF/INDEX.LIST",
            )
            excludes += listOf(
                "META-INF/DEPENDENCIES",
                "META-INF/LICENSE*",
                "META-INF/NOTICE*",
            )
        }
    }
}

protobuf {
    protoc {
        artifact = "com.google.protobuf:protoc:3.25.3"
    }
    plugins {
        id("grpc") {
            artifact = "io.grpc:protoc-gen-grpc-java:1.63.0"
        }
    }
    generateProtoTasks {
        all().forEach { task ->
            // Generate `protobuf-javalite` message classes and match the gRPC
            // stubs to them (`lite`), the supported Android combination.
            task.builtins {
                id("java") {
                    option("lite")
                }
            }
            task.plugins {
                id("grpc") {
                    option("lite")
                }
            }
        }
    }
}

dependencies {
    implementation("io.grpc:grpc-netty:1.63.0")
    implementation("io.grpc:grpc-protobuf-lite:1.63.0")
    implementation("io.grpc:grpc-stub:1.63.0")
    implementation("com.google.protobuf:protobuf-javalite:3.25.3")

    // `@javax.annotation.Generated` is referenced by the generated gRPC stubs.
    compileOnly("org.apache.tomcat:annotations-api:6.0.53")

    testImplementation("junit:junit:4.13.2")
    // In-process gRPC transport (split out of grpc-core in 1.60) so the service
    // can be exercised on the JVM without a device or a real socket.
    testImplementation("io.grpc:grpc-inprocess:1.63.0")
}
