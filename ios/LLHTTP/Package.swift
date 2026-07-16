// swift-tools-version:5.9
import PackageDescription

// Local package vendoring llhttp (nodejs/llhttp v9.4.2) — the same battle-tested
// HTTP/1 parser SwiftNIO wraps as CNIOLLHTTP and Node.js uses — as a clean,
// streaming, event-loop-free C dependency. `CLLHTTP` is the raw C; `LLHTTP` is a
// thin Swift wrapper (`HTTP1Parser`) over it.
let package = Package(
    name: "LLHTTP",
    platforms: [.iOS(.v15), .macOS(.v12)],
    products: [
        .library(name: "LLHTTP", targets: ["LLHTTP"]),
    ],
    targets: [
        .target(name: "CLLHTTP", exclude: ["LICENSE-MIT", "VENDORING.md"]),
        .target(name: "LLHTTP", dependencies: ["CLLHTTP"]),
        .testTarget(name: "LLHTTPTests", dependencies: ["LLHTTP"]),
    ]
)
