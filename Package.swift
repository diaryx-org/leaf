// swift-tools-version:5.9
//
// The Swift package an AppKit/SwiftUI app links to drive leaf-core. The manifest
// lives at the repository root — not in packages/leaf-swift/ — because SwiftPM
// resolves git dependencies only from a root Package.swift, and by-version
// resolution is how a consumer is meant to take this package:
//
//   .package(url: "https://github.com/diaryx-org/leaf.git", from: "X.Y.Z")
//
// (A local checkout still works: `.package(path: "…/leaf")` — the repo root, not
// packages/leaf-swift/.) The sources stay under packages/leaf-swift/; only the
// manifest sits here.
//
// It builds the UniFFI binding + the LeafUI renderer **from source**; the Rust
// staticlib itself is linked by the consuming app via a `-force_load` linker
// flag and (re)built by a pre-build step — see `apps/leaf-editor/project.yml`,
// which does exactly that so an Xcode build always picks up fresh Rust changes
// (a prebuilt xcframework would be cached instead):
//   OTHER_LDFLAGS = -force_load <path>/libleaf_ffi.a
// (`scripts/build-xcframework.sh` still exists to produce a *distributable*
// prebuilt xcframework, but the package no longer depends on one.)
//
// The two `uniffi-generated/` inputs below are committed — a version-resolved
// clone runs no generators, so they must build as-is. `scripts/gen-bindings.sh`
// writes them from crates/leaf-ffi and CI holds them to it (`--check`):
//
//   • packages/leaf-swift/uniffi-generated/headers/           the C ABI header +
//     module map (the `leaf_ffiFFI` clang module the generated Swift imports)
//   • packages/leaf-swift/uniffi-generated/Sources/LeafFFI/   the UniFFI-
//     generated Swift over that C ABI
import PackageDescription

let package = Package(
    name: "LeafFFI",
    platforms: [.macOS(.v12), .iOS(.v16)],
    products: [
        // The low-level binding: `LeafDoc` + the `DocView`/`Row`/`Run` value types.
        .library(name: "LeafFFI", targets: ["LeafFFI"]),
        // The AppKit/SwiftUI renderer built on it: `LeafEditor` + `LeafEditorModel`.
        .library(name: "LeafUI", targets: ["LeafUI"]),
    ],
    targets: [
        // The C ABI as a clang module (`import leaf_ffiFFI`). No library to link
        // here — the app force-loads the Rust `.a`, so the symbols the generated
        // Swift references stay undefined until the final executable link.
        .systemLibrary(
            name: "leaf_ffiFFI",
            path: "packages/leaf-swift/uniffi-generated/headers"
        ),
        // The generated Swift, compiled against that C module.
        .target(
            name: "LeafFFI",
            dependencies: ["leaf_ffiFFI"],
            path: "packages/leaf-swift/uniffi-generated/Sources/LeafFFI"
        ),
        // The reusable AppKit/SwiftUI editor surface (committed source).
        .target(
            name: "LeafUI",
            dependencies: ["LeafFFI"],
            path: "packages/leaf-swift/Sources/LeafUI"
        ),
        // Renderer unit tests. They build `Row`/`DocView` fixtures in pure Swift and
        // exercise the CoreText geometry + attribute mapping — no `LeafDoc`/Rust
        // calls — but the module still references the FFI symbols, so the test
        // binary must link the staticlib. `scripts/test-swift.sh` force-loads it;
        // see that script (plain `swift test` won't find the `.a`).
        .testTarget(
            name: "LeafUITests",
            dependencies: ["LeafUI"],
            path: "packages/leaf-swift/Tests/LeafUITests"
        ),
    ]
)
