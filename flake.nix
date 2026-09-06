{
  description = "replicator - Tauri 2 + React/TS dev shell";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
    rust-overlay = {
      url = "github:oxalica/rust-overlay";
      inputs.nixpkgs.follows = "nixpkgs";
    };
  };

  outputs = { self, nixpkgs, flake-utils, rust-overlay }:
    flake-utils.lib.eachDefaultSystem (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ rust-overlay.overlays.default ];
        };

        # llvm-tools-preview provides llvm-profdata/llvm-cov, which
        # cargo-llvm-cov shells out to for coverage reports.
        rustToolchain = pkgs.rust-bin.stable.latest.default.override {
          extensions = [ "rust-src" "rust-analyzer" "clippy" "rustfmt" "llvm-tools-preview" ];
        };

        # Tauri 2 on Linux needs webkitgtk 4.1 + libsoup 3.
        linuxDeps = with pkgs; lib.optionals stdenv.isLinux [
          webkitgtk_4_1
          libsoup_3
          gtk3
          glib
          gobject-introspection
          librsvg
          libayatana-appindicator
          xdotool
          # GStreamer - webkitgtk uses it for HTML5 audio + Web Audio API
          gst_all_1.gstreamer
          gst_all_1.gst-plugins-base
          gst_all_1.gst-plugins-good
          gst_all_1.gst-plugins-bad
          gst_all_1.gst-libav
          # Runtime audio server libs so autoaudiosink can find a real sink
          libpulseaudio
          pipewire
          alsa-lib
        ];

        gstPluginPath = pkgs.lib.makeSearchPath "lib/gstreamer-1.0" linuxDeps;

        darwinDeps = with pkgs; lib.optionals stdenv.isDarwin (
          with darwin.apple_sdk.frameworks; [
            AppKit
            CoreServices
            Security
            WebKit
          ]
        );
      in
      {
        devShells.default = pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
            rustToolchain
            cargo-tauri
            cargo-llvm-cov
            nodejs_22
            pnpm
            typescript
            typescript-language-server
          ];

          buildInputs = with pkgs; [
            openssl
          ] ++ linuxDeps ++ darwinDeps;

          # rusqlite "bundled" feature needs a working C toolchain - provided
          # by stdenv. Keep RUST_SRC_PATH so rust-analyzer finds std sources.
          env = {
            RUST_SRC_PATH = "${rustToolchain}/lib/rustlib/src/rust/library";
          };

          shellHook = ''
            # Set this here because nix develop drops the build-time TZ value.
            export TZ=UTC
            # Help Tauri/webkitgtk find GIO modules at runtime on NixOS.
            export GIO_MODULE_DIR="${pkgs.glib-networking}/lib/gio/modules/"
            export XDG_DATA_DIRS="${pkgs.gsettings-desktop-schemas}/share/gsettings-schemas/${pkgs.gsettings-desktop-schemas.name}:${pkgs.gtk3}/share/gsettings-schemas/${pkgs.gtk3.name}:$XDG_DATA_DIRS"
            # GStreamer plugin search path - webkitgtk needs this for Web Audio / HTML5 audio
            export GST_PLUGIN_SYSTEM_PATH_1_0="${gstPluginPath}"
          '';
        };

        formatter = pkgs.nixpkgs-fmt;
      });
}
