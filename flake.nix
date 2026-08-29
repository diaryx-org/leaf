{
  description = "leaf — a caret-based rich-text document editor CLI and library";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    # The org's Rust and Zig pins, and the shell that carries them. leaf is the
    # repository that sets the Rust one: gpui uses library features stabilised in
    # 1.95, so `versions.rust` there and `rust-toolchain.toml` here are the same
    # number by construction.
    diaryx-nix.url = "github:diaryx-org/nix";
    diaryx-nix.inputs.nixpkgs.follows = "nixpkgs";
  };

  outputs = { self, nixpkgs, flake-utils, diaryx-nix }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs { inherit system; };
        zig = diaryx-nix.lib.${system}.zig;

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
          # meant fetching the Zed monorepo. That lockfile now has no git
          # entries at all; see the `exclude` note in Cargo.toml.
          leaf = pkgs.rustPlatform.buildRustPackage {
            pname = "leaf";
            inherit version;
            src = ./.;

            cargoLock.lockFile = ./Cargo.lock;

            # zig for twig-sys's build.rs. It only actually runs on a target with
            # no prebuilt payload crate — of the four systems this flake covers,
            # that is x86_64-darwin alone; the other three link twig-sys's
            # shipped archive and never invoke it. Unconditional anyway, because
            # a toolchain that is present and unused costs a closure entry, and
            # one that is absent costs a build failure on one system only, found
            # by whoever happens to be on it. On Apple targets that build script
            # also repacks Zig's static archive with `libtool` (ld64 rejects
            # Zig's alignment); cctools provides that `libtool`, which is not
            # otherwise on the sandbox PATH.
            nativeBuildInputs = [ zig ]
              ++ pkgs.lib.optionals pkgs.stdenv.hostPlatform.isDarwin [ pkgs.cctools ];

            # Those Zig builds want a writable HOME + cache dirs, which the
            # read-only Nix store will not provide.
            preBuild = ''
              export HOME="$TMPDIR"
              export ZIG_GLOBAL_CACHE_DIR="$TMPDIR/zig-global-cache"
              export ZIG_LOCAL_CACHE_DIR="$TMPDIR/zig-local-cache"
            '';

            # The twig-sys build script repacks its Zig archive with
            # `libtool`/`ar`, which leaves an unreadable `__.SYMDEF` in the build
            # script's `out/repack` dir. buildRustPackage's install hook then
            # does a bulk `cp -r` of the release dir and fails on it. This runs
            # before that hook (the postBuild attr precedes postBuildHooks), so
            # make the tree readable first.
            postBuild = ''
              chmod -R u+rwX target
            '';

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

        # The shared rust+zig shell: the pinned Rust toolchain rather than
        # nixpkgs' `cargo`/`rustc`, the Zig twig-sys falls back to, and git-cliff
        # for `dx changelog`.
        devShells.default = diaryx-nix.devShells.${system}.rust-zig;
      });
}
