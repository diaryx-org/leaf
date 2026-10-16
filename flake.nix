{
  description = "leaf — a caret-based rich-text document editor CLI and library";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The org's Rust pin, and the shell that carries it. leaf is the
    # repository that sets it: gpui uses library features stabilised in
    # 1.95, so `versions.rust` there and `rust-toolchain.toml` here are the same
    # number by construction.
    diaryx-nix.url = "github:diaryx-org/nix";
    diaryx-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, diaryx-nix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        # The workspace version (single source of truth in [workspace.package]).
        # Parse it so the flake reports the same number as `leaf --version`.
        version =
          let m = builtins.match ".*\n[[:blank:]]*version = \"([^\"]+)\".*"
                    (builtins.readFile ./Cargo.toml);
          in if m == null
             then throw "leaf flake: could not find workspace version in Cargo.toml"
             else builtins.head m;
      in {
        packages = rec {
          default = leaf;

          # Packaging this at all is what moved the gpui crates out of the
          # workspace. `buildRustPackage` vendors from Cargo.lock and demands an
          # output hash for every git-sourced entry there whatever `-p` it is
          # told to build — so while leaf-gpui was a member, building the TUI
          # meant fetching the Zed monorepo; see the `exclude` note in
          # Cargo.toml. What git entries it still has are small (fig-css, the
          # mermaid renderer's fork, resvg-swift), pinned by rev in Cargo.lock.
          leaf = pkgs.rustPlatform.buildRustPackage {
            pname = "leaf";
            inherit version;
            src = ./.;

            cargoLock = {
              lockFile = ./Cargo.lock;
              # The git entries are fetched by the rev Cargo.lock pins, which
              # needs no hash written here.
              allowBuiltinFetchGit = true;
            };

            # twig (6) is pure Rust, so the build needs nothing but the
            # toolchain: no Zig, and no `libtool` to repack its archive.

            # Build/test only the CLI crate; leaf-core and leaf-ratatui come in
            # as path dependencies. leaf-ffi and leaf-wasm are not in this graph.
            cargoBuildFlags = [ "-p" "leaf-tui" ];
            cargoTestFlags = [ "-p" "leaf-tui" ];

            meta = {
              description = "Caret-based rich-text terminal editor for Markdown, Djot, HTML, and XML";
              homepage = "https://github.com/diaryx-org/leaf";
              license = with pkgs.lib.licenses; [ mit asl20 ];
              mainProgram = "leaf";
              platforms = pkgs.lib.platforms.unix;
            };
          };
        };

        apps.default = {
          type = "app";
          program = "${self.packages.${system}.leaf}/bin/leaf";
        };

        # The shared Rust shell: the pinned Rust toolchain rather than
        # nixpkgs' `cargo`/`rustc`, and git-cliff for `ship changelog`.
        devShells.default = diaryx-nix.devShells.${system}.rust;
      });
}
