// swift-tools-version: 5.9
import PackageDescription

// The live engine for the Apple apps: capture, rendering, the loop, the link
// to the virtual camera, and the face swap that runs in the shared core.
//
// The core arrives as a static library and a generated Swift file. Both are
// built by tools/build-core-apple.sh and are not kept in git.
let package = Package(
    name: "GodsCameraKit",
    platforms: [.macOS("27.0")],
    products: [
        .library(name: "GodsCameraKit", targets: ["GodsCameraKit"])
    ],
    targets: [
        .binaryTarget(name: "GCCoreFFI", path: "Frameworks/GCCoreFFI.xcframework"),
        .target(
            name: "GCCore",
            dependencies: ["GCCoreFFI"],
            // ONNX Runtime, inside the core, needs Core ML and the C++ library.
            linkerSettings: [.linkedFramework("CoreML"), .linkedLibrary("c++")]
        ),
        .target(name: "GodsCameraKit", dependencies: ["GCCore"]),
        .testTarget(name: "GodsCameraKitTests", dependencies: ["GodsCameraKit"]),
    ]
)
