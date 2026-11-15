{
  description = "Termide - All-in-one terminal workspace: editor, file manager, terminal, git and coding agent";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
    # Used by ./default.nix to expose the flake's overlay to legacy
    # (non-flake) Nix consumers such as NixOS configurations that pin
    # nixpkgs via channels and import the repo via `fetchTarball`.
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils, ... }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Desktop targets for cross-platform terminal application.
        # The musl entries are what make `nix build .#termide-static` work.
        desktopTargets = if pkgs.stdenv.isDarwin then [
          "x86_64-apple-darwin"
          "aarch64-apple-darwin"
        ] else [
          "x86_64-unknown-linux-gnu"
          "aarch64-unknown-linux-gnu"
          "x86_64-unknown-linux-musl"
          "aarch64-unknown-linux-musl"
          "x86_64-pc-windows-gnu"
        ];

        # Single source of truth for the compiler version: rust-toolchain.toml,
        # which is also what rustup honours for a non-Nix checkout and what the
        # pinned dtolnay/rust-toolchain step in CI matches. Reading it here
        # keeps `nix develop` from silently drifting onto a newer stable than
        # the one PRs are gated on.
        rustChannel = (builtins.fromTOML (builtins.readFile ./rust-toolchain.toml)).toolchain.channel;

        rustToolchain = pkgs.rust-bin.stable.${rustChannel}.default.override {
          extensions = [ "rust-src" "rust-analyzer" ];
          targets = desktopTargets;
        };

        nativeBuildInputs = with pkgs; [
          # Rust toolchain
          rustToolchain

          # Build tools
          pkg-config

          # Code quality tools
          cargo-audit
          cargo-outdated
        ] ++ lib.optionals stdenv.isLinux [
          # Coverage via ptrace — Linux-only, and not packaged for Darwin.
          cargo-tarpaulin

          # C compiler for the tree-sitter grammars. On Darwin the stdenv
          # already provides clang; pulling GCC in there fights the `CC = "cc"`
          # below instead of helping.
          gcc
        ];
        # Note: mingw cross-compiler removed to avoid CC conflicts with tree-sitter
        # For Windows builds, use native Windows environment or GitHub Actions

        # tree-sitter grammars compile native C in build.rs — pkg-config and a
        # working C toolchain in nativeBuildInputs are enough. Darwin needs no
        # explicit frameworks either: the workspace is pure Rust (rustls/russh,
        # no OpenSSL), and current nixpkgs ships the Apple SDK as part of the
        # stdenv rather than as `darwin.apple_sdk.frameworks.*` attributes.
        buildInputs = [ ];

        # The default package builds with the same pinned compiler as the dev
        # shell, without its extensions and extra targets.
        rustMinimal = pkgs.rust-bin.stable.${rustChannel}.minimal;
        rustPlatform = pkgs.makeRustPlatform {
          cargo = rustMinimal;
          rustc = rustMinimal;
        };

        # What the default and the static package share.
        packageArgs = {
          pname = "termide";
          version = "0.49.0";

          src = ./.;
          cargoLock.lockFile = ./Cargo.lock;

          # GCC miscompiles tree-sitter's array.h under strict aliasing; the
          # deb, rpm, AUR and install.sh builds pass the same flag.
          env.NIX_CFLAGS_COMPILE = "-fno-strict-aliasing";
        };

      in
      {
        packages = {
          default = rustPlatform.buildRustPackage (packageArgs // {
            nativeBuildInputs = [ pkgs.pkg-config pkgs.installShellFiles ];

            postInstall = ''
              installShellCompletion --cmd termide \
                --bash completions/termide.bash \
                --zsh completions/_termide \
                --fish completions/termide.fish
            '';

            meta = with pkgs.lib; {
              description = "All-in-one terminal workspace: editor, file manager, terminal, git and coding agent";
              homepage = "https://github.com/termide/termide";
              license = licenses.mit;
              maintainers = [ ];
              mainProgram = "termide";
              platforms = platforms.unix;
            };
          });

          termide = self.packages.${system}.default;
        } // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          # Fully static musl build — a single self-contained binary
          # that runs on any Linux distro, including Alpine.
          #
          # Possible only because the workspace is pure-Rust end-to-end:
          # FTPS uses rustls + webpki-roots, SFTP uses russh +
          # russh-sftp (no OpenSSL / no libssh2). Tree-sitter grammars
          # are compiled by build.rs against musl-gcc. pkgsStatic targets
          # the musl flavour of the host's own architecture, so this builds
          # on x86_64 and aarch64 alike. It takes nixpkgs' own compiler:
          # the pinned rust-overlay toolchain's build scripts segfault in
          # pkgsStatic's cross setup.
          termide-static = pkgs.pkgsStatic.rustPlatform.buildRustPackage (packageArgs // {
            doCheck = false;
          });
        };

        apps = {
          default = {
            type = "app";
            program = "${self.packages.${system}.default}/bin/termide";
          };
        } // pkgs.lib.optionalAttrs pkgs.stdenv.isLinux {
          termide-static = {
            type = "app";
            program = "${self.packages.${system}.termide-static}/bin/termide";
          };
        };

        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;

          shellHook = ''
            echo "🦀 Development environment"
          '';

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          # Ensure tree-sitter uses native compiler, not mingw cross-compiler
          CC = "cc";
        };
      }) // {
        overlays.default = final: prev: {
          termide = self.packages.${final.system}.default;
        };
      };
}
