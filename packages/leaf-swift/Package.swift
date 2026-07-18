// swift-tools-version:5.9
//
// The Swift package an AppKit/SwiftUI app links to drive leaf-core. It builds the
// UniFFI binding + the LeafUI renderer **from source**; the Rust staticlib itself
// is linked by the consuming app via a `-force_load` linker flag and (re)built by
// a pre-build step — see `apps/leaf-ffi-ios/project.yml`, which does exactly that so an
// Xcode build always picks up fresh Rust changes (a prebuilt xcframework would be
// cached instead). `bootstrap.sh` generates the two `generated/` inputs below.
//
//   • generated/headers/            the C ABI header + module map (the `leaf_ffiFFI`
//                                   clang module the generated Swift imports)
//   • generated/Sources/LeafFFI/    the UniFFI-generated Swift over that C ABI
//
// A consumer adds this directory as a local package and links the staticlib:
//   .package(path: "…/packages/leaf-swift")        // import LeafUI
//   OTHER_LDFLAGS = -force_load <path>/libleaf_ffi.a
// (`scripts/build-xcframework.sh` still exists to produce a *distributable*
// prebuilt xcframework, but the package no longer depends on one.)
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
        .systemLibrary(name: "leaf_ffiFFI", path: "generated/headers"),
        // The generated Swift, compiled against that C module.
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
    ]
)
