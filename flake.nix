{
  description = "TermIDE - Cross-platform terminal IDE, file manager and virtual terminal";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    rust-overlay.url = "github:oxalica/rust-overlay";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, rust-overlay, flake-utils }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        overlays = [ (import rust-overlay) ];
        pkgs = import nixpkgs {
          inherit system overlays;
        };

        # Desktop targets for cross-platform terminal application
        # Supports Linux, macOS, and Windows (via WSL/MinGW)
        desktopTargets = if pkgs.stdenv.isDarwin then [
          "x86_64-apple-darwin"
          "aarch64-apple-darwin"
        ] else [
          "x86_64-unknown-linux-gnu"
          "aarch64-unknown-linux-gnu"
          "x86_64-pc-windows-gnu"
        ];

        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
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
          cargo-tarpaulin

          # Native compilation tools
          gcc
        ];
        # Note: mingw cross-compiler removed to avoid CC conflicts with tree-sitter
        # For Windows builds, use native Windows environment or GitHub Actions

        buildInputs = with pkgs; [
          # Required for some terminal/crypto crates
          openssl
          # Required for tree-sitter C compilation
          # (oniguruma removed - was for syntect)
        ] ++ lib.optionals stdenv.isDarwin [
          # macOS frameworks
          darwin.apple_sdk.frameworks.Security
          darwin.apple_sdk.frameworks.SystemConfiguration
        ];

      in
      {
        devShells.default = pkgs.mkShell {
          inherit nativeBuildInputs buildInputs;

          shellHook = ''
            echo "🦀 Development environment"
          '';

          RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          # Ensure tree-sitter uses native compiler, not mingw cross-compiler
          CC = "cc";
        };
      });
}
