// swift-tools-version:5.9
//
// The Swift package an AppKit/SwiftUI app links to drive leaf-core. It stitches
// together the two products of `scripts/build-xcframework.sh` (both under
// `generated/`, git-ignored — run that script once before building):
//
//   • LeafFFI.xcframework   the Rust staticlib (C ABI) for every Apple slice
//   • Sources/LeafFFI/…     the UniFFI-generated Swift over that C ABI
//
// Consume it from an app by adding this directory as a local package:
//   .package(path: "../leaf/crates/leaf-ffi")
// then `import LeafFFI` and construct `try LeafDoc(source:, format:)`.
import PackageDescription

let package = Package(
    name: "LeafFFI",
    platforms: [.macOS(.v12), .iOS(.v15)],
    products: [
        // The low-level binding: `LeafDoc` + the `DocView`/`Row`/`Run` value types.
        .library(name: "LeafFFI", targets: ["LeafFFI"]),
        // The AppKit/SwiftUI renderer built on it: `LeafEditor` + `LeafEditorModel`.
        .library(name: "LeafUI", targets: ["LeafUI"]),
    ],
    targets: [
        // The generated Swift, compiled against the C shim inside the xcframework.
        .target(
            name: "LeafFFI",
            dependencies: ["leaf_ffiFFI"],
            path: "generated/Sources/LeafFFI"
        ),
        // The reusable AppKit/SwiftUI editor surface (committed source).
        .target(
            name: "LeafUI",
            dependencies: ["LeafFFI"],
            path: "Sources/LeafUI"
        ),
        // The prebuilt Rust core, C ABI + headers, one binary for macOS + iOS.
        .binaryTarget(
            name: "leaf_ffiFFI",
            path: "generated/LeafFFI.xcframework"
        ),
    ]
)
