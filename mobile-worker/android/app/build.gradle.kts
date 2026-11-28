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

    buildFeatures {
        compose = true
    }

    // Task 10/12 Compose toolchain. Kotlin 1.9.24 pairs with compiler 1.5.14.
    composeOptions {
        kotlinCompilerExtensionVersion = "1.5.14"
    }

    testOptions {
        unitTests.isIncludeAndroidResources = true
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
    // Compose (Task 12 block, installed here because Task 10 ships the UI).
    val composeBom = platform("androidx.compose:compose-bom:2024.09.02")
    implementation(composeBom)
    androidTestImplementation(composeBom)
    implementation("androidx.activity:activity-compose:1.9.2")
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.6")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.6")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.8.1")
    androidTestImplementation("androidx.compose.ui:ui-test-junit4")
    debugImplementation("androidx.compose.ui:ui-test-manifest")

    implementation("io.grpc:grpc-netty:1.63.0")

    // The control/pairing screen uses a Material3 theme; without this the
    // resource linker cannot resolve Theme.Material3.* parents.
    implementation("com.google.android.material:material:1.12.0")

    // Keystore-backed storage for the LAN bearer token and provider keys.
    implementation("androidx.security:security-crypto:1.1.0-alpha06")

    // Not optional: gRPC over TLS on Android cannot work without this.
    //
    // grpc-netty needs an ALPN provider to negotiate HTTP/2, and on Android it
    // looks for Conscrypt under the standalone `org.conscrypt` package. The
    // platform's Conscrypt lives in `com.android.org.conscrypt`, so every
    // provider check in `GrpcSslContexts.findJdkProvider` fails and it throws
    // "Could not find TLS ALPN provider; no working netty-tcnative, Conscrypt,
    // or Jetty NPN/ALPN available". netty is even more specific: its Android
    // branch instantiates `org.conscrypt.OpenSSLProvider` by name.
    //
    // This artifact supplies that package (with arm64-v8a, armeabi-v7a, x86 and
    // x86_64 native libraries), which is what makes the mTLS worker port able
    // to speak HTTP/2 at all.
    implementation("org.conscrypt:conscrypt-android:2.8.0")

    implementation("io.grpc:grpc-protobuf-lite:1.63.0")
    implementation("io.grpc:grpc-stub:1.63.0")
    implementation("com.google.protobuf:protobuf-javalite:3.25.3")

    // `@javax.annotation.Generated` is referenced by the generated gRPC stubs.
    compileOnly("org.apache.tomcat:annotations-api:6.0.53")

    testImplementation("junit:junit:4.13.2")
    // In-process gRPC transport (split out of grpc-core in 1.60) so the service
    // can be exercised on the JVM without a device or a real socket.
    testImplementation("io.grpc:grpc-inprocess:1.63.0")

    // LAN chat transport; org.json is supplied by Android in production and by
    // this test artifact on the JVM.
    implementation("com.squareup.okhttp3:okhttp:4.12.0")
    testImplementation("org.json:json:20240303")
}
