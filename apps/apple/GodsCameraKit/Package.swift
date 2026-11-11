// swift-tools-version:5.9
import PackageDescription

let package = Package(
    name: "GodsCameraKit",
    platforms: [.macOS("27.0")],
    products: [.library(name: "GodsCameraKit", targets: ["GodsCameraKit"])],
    targets: [
        .target(name: "GodsCameraKit"),
        .testTarget(name: "GodsCameraKitTests", dependencies: ["GodsCameraKit"]),
    ]
)
