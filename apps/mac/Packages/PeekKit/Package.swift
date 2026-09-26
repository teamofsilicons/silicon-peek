// swift-tools-version: 6.2
// 6.2 is required for `.macOS(.v26)` (BLUEPRINT §8.1). Swift 6 language mode throughout.
//
// Module ownership and dependency rules are in apps/mac/ARCHITECTURE.md. In short:
//   PeekCore      protocol, models, geometry, input snapshot, interfaces (frozen; no AppKit UI)
//   PeekIPC       DaemonLink over the peekd Unix socket
//   PeekDrawing   QuickJS drawing runtime, compositor, validator        (depends on CQuickJS)
//   PeekAudio     TTS playback and mic capture
//   PeekInput     InputHub, backdrop, appearance, pointer, image cache
//   PeekUI        slots, chrome, hotkeys, coordinator, settings, simulation, menu bar
import PackageDescription

let swiftSettings: [SwiftSetting] = [
    .enableUpcomingFeature("ExistentialAny")
]

let package = Package(
    name: "PeekKit",
    platforms: [.macOS(.v26)],
    products: [
        .library(name: "PeekCore", targets: ["PeekCore"]),
        .library(name: "PeekIPC", targets: ["PeekIPC"]),
        .library(name: "PeekDrawing", targets: ["PeekDrawing"]),
        .library(name: "PeekAudio", targets: ["PeekAudio"]),
        .library(name: "PeekInput", targets: ["PeekInput"]),
        .library(name: "PeekUI", targets: ["PeekUI"]),
    ],
    dependencies: [
        .package(path: "../CQuickJS")
    ],
    targets: [
        .target(name: "PeekCore", swiftSettings: swiftSettings),
        .target(name: "PeekIPC", dependencies: ["PeekCore"], swiftSettings: swiftSettings),
        .target(
            name: "PeekDrawing",
            dependencies: ["PeekCore", .product(name: "CQuickJS", package: "CQuickJS")],
            swiftSettings: swiftSettings),
        .target(name: "PeekAudio", dependencies: ["PeekCore"], swiftSettings: swiftSettings),
        .target(name: "PeekInput", dependencies: ["PeekCore"], swiftSettings: swiftSettings),
        .target(
            name: "PeekUI",
            dependencies: ["PeekCore", "PeekIPC", "PeekDrawing", "PeekAudio", "PeekInput"],
            swiftSettings: swiftSettings),

        .testTarget(name: "PeekCoreTests", dependencies: ["PeekCore"], swiftSettings: swiftSettings),
        .testTarget(name: "PeekIPCTests", dependencies: ["PeekIPC", "PeekCore"], swiftSettings: swiftSettings),
        .testTarget(name: "PeekDrawingTests", dependencies: ["PeekDrawing", "PeekCore"], swiftSettings: swiftSettings),
        .testTarget(name: "PeekAudioTests", dependencies: ["PeekAudio", "PeekCore"], swiftSettings: swiftSettings),
        .testTarget(
            name: "PeekInputTests", dependencies: ["PeekInput", "PeekAudio", "PeekCore"], swiftSettings: swiftSettings),
        .testTarget(
            name: "PeekUITests", dependencies: ["PeekUI", "PeekCore", "PeekIPC"], swiftSettings: swiftSettings),
    ],
    swiftLanguageModes: [.v6]
)
