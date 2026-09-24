// swift-tools-version: 5.9
//
// The Wispers Access SDK for Swift: the UniFFI-generated API over the Rust
// library. Both the XCFramework and the Swift source are produced by
// build-xcframework.sh next to this file and are not checked in; run it
// once before opening an app that depends on this package.

import PackageDescription

let package = Package(
    name: "WispersAccessSdk",
    platforms: [.iOS(.v17), .macOS(.v14)],
    products: [
        .library(name: "WispersAccessSdk", targets: ["WispersAccessSdk"]),
    ],
    targets: [
        .binaryTarget(
            name: "WispersAccessSdkFfi",
            path: "WispersAccessSdkFfi.xcframework"
        ),
        .target(
            name: "WispersAccessSdk",
            dependencies: ["WispersAccessSdkFfi"],
            path: "Sources/WispersAccessSdk",
            linkerSettings: [
                // What the native dependencies inside the Rust library need:
                // BoringSSL and libjuice the libraries, iroh's network
                // monitor the framework.
                .linkedLibrary("c++"),
                .linkedLibrary("iconv"),
                .linkedLibrary("resolv"),
                .linkedFramework("SystemConfiguration"),
            ]
        ),
        .testTarget(
            name: "WispersAccessSdkTests",
            dependencies: ["WispersAccessSdk"]
        ),
    ]
)
