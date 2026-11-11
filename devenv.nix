# Development environment for x86_64 NixOS (and other Linux): `devenv shell`, then `cargo run`.
# On macOS, use mise and .tool-versions instead.
{ pkgs, lib, ... }:

let
  # winit and wgpu load these at run time with dlopen.
  runtimeLibs = with pkgs; [
    libxkbcommon
    wayland
    vulkan-loader
    libGL
    libx11
    libxcursor
    libxi
    libxrandr
  ];
in
{
  languages.rust.enable = true;

  packages =
    with pkgs;
    [
      pkg-config
      # mdwriter runs these at runtime.
      git
      gh
      curl
      openssh
    ]
    ++ lib.optionals stdenv.isLinux ([ fontconfig ] ++ runtimeLibs);

  env = lib.optionalAttrs pkgs.stdenv.isLinux {
    LD_LIBRARY_PATH = lib.makeLibraryPath runtimeLibs;
  };

  enterTest = ''
    cargo test
  '';
}
