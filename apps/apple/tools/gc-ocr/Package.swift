// swift-tools-version: 5.9
import PackageDescription

// A small command-line tool that reads text from pictures with Apple's Vision
// framework and prints it as JSON. The hub runs it for document jobs; on
// Windows a different engine takes its place behind the same output.
let package = Package(
    name: "gc-ocr",
    platforms: [.macOS("27.0")],
    targets: [.executableTarget(name: "gc-ocr")]
)
