# gRPC / Netty / protobuf reflection-free lite runtime.
-keep class io.grpc.** { *; }
-keep class io.netty.** { *; }
-keep class com.google.protobuf.** { *; }
-dontwarn io.grpc.**
-dontwarn io.netty.**
-dontwarn com.google.protobuf.**
-dontwarn javax.annotation.**
