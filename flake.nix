{
  description = "leaf — a caret-based rich-text document editor";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The org's Rust and Zig pins, and the shell that carries them. leaf is the
    # repository that sets the Rust one: gpui, pulled from the Zed monorepo by
    # leaf-gpui, uses library features stabilised in 1.95, so `versions.rust`
    # there and `rust-toolchain.toml` here are the same number by construction.
    diaryx-nix.url = "github:diaryx-org/nix";
    diaryx-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, diaryx-nix }:
    flake-utils.lib.eachDefaultSystem (system:
      {
        # The shared rust+zig shell. Zig is here for the same reason it is in
        # flower's and prov's: twig-sys compiles libtwig.a from vendored source
        # on any target it ships no prebuilt payload for, which among the systems
        # this flake covers is x86_64-darwin.
        #
        # No `packages` yet, deliberately, and it is not an oversight. Packaging
        # the `leaf` binary needs `buildRustPackage`, which vendors from
        # Cargo.lock and demands an output hash for every git-sourced entry in it
        # — regardless of which `-p` is being built. This lockfile has 26 such
        # entries, 22 of them the Zed monorepo, because `crates/leaf-gpui`
        # depends on gpui directly. So a packaged `leaf` would fetch gigabytes of
        # gpui to produce a terminal binary that links none of it: 769 packages
        # vendored for the 141 `leaf-tui` actually uses.
        #
        # Excluding `apps/leaf` from the workspace does not fix this — measured:
        # 786 packages and the same 26 git entries, since leaf-gpui is the crate
        # that pulls gpui, not the app. Moving *both* leaf-gpui and apps/leaf out
        # gets there (275 packages, no git entries), but that is half the gpui
        # side of the project leaving `--workspace`, and it is a decision about
        # this repository's shape rather than about its flake.
        devShells.default = diaryx-nix.devShells.${system}.rust-zig;
      });
}
