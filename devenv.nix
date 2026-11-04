# Development environment for x86_64 NixOS (and other Linux): `devenv shell`, then `pnpm install` and
# `cargo tauri dev`. On macOS, use mise and .tool-versions instead.
{ pkgs, lib, ... }:

{
  languages.rust.enable = true;
  languages.javascript = {
    enable = true;
    package = pkgs.nodejs_26;
    pnpm = {
      enable = true;
      package = pkgs.pnpm_12;
    };
  };

  packages =
    with pkgs;
    [
      cargo-tauri
      pkg-config
      gobject-introspection
      # mdwriter runs these at runtime.
      git
      gh
      curl
      openssh
    ]
    # Tauri's Linux libraries: https://v2.tauri.app/start/prerequisites/#linux
    ++ lib.optionals stdenv.isLinux [
      at-spi2-atk
      atkmm
      cairo
      gdk-pixbuf
      glib
      glib-networking
      gtk3
      harfbuzz
      librsvg
      libsoup_3
      pango
      webkitgtk_4_1
      openssl
    ];

  env = {
    # package.json pins pnpm; without this pnpm downloads that exact version as a prebuilt binary, which
    # NixOS can't run. The pnpm from nixpkgs reads the same lockfile.
    npm_config_manage_package_manager_versions = "false";
  }
  // lib.optionalAttrs pkgs.stdenv.isLinux {
    # WebKitGTK shows a blank window on some GPU drivers (often NVIDIA) without this.
    WEBKIT_DISABLE_DMABUF_RENDERER = "1";
    # HTTPS from the webview needs the GIO TLS module.
    GIO_MODULE_DIR = "${pkgs.glib-networking}/lib/gio/modules/";
  };

  enterTest = ''
    cargo test --manifest-path src-tauri/Cargo.toml --lib
    pnpm typecheck
  '';
}
