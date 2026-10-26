{
  description = "leaf — a caret-based rich-text document editor";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The org's Rust pin, and the shell that carries it: `versions.rust` there
    # and `rust-toolchain.toml` here are the same number.
    diaryx-nix.url = "github:diaryx-org/nix";
    diaryx-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { nixpkgs, flake-utils, diaryx-nix, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      {
        # No package: the terminal editor this flake built, leaf-tui, is
        # archived (archive/README.md). Its successor, apps/leaf-dioxus-tui,
        # is not packaged here yet.

        # The shared Rust shell: the pinned Rust toolchain rather than
        # nixpkgs' `cargo`/`rustc`, and git-cliff for `ship changelog`.
        devShells.default = diaryx-nix.devShells.${system}.rust;
      });
}
