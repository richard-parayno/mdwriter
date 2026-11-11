{
  lib,
  rustPlatform,
  pkg-config,
  makeWrapper,
  fontconfig,
  libxkbcommon,
  wayland,
  vulkan-loader,
  libGL,
  libx11,
  libxcursor,
  libxi,
  libxrandr,
  git,
  gh,
  curl,
  openssh,
}:

let
  # winit and wgpu load these at run time with dlopen, so they aren't in the binary's RPATH.
  runtimeLibs = [
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
rustPlatform.buildRustPackage {
  pname = "mdwriter";
  version = (lib.importTOML ../Cargo.toml).package.version;

  src = lib.fileset.toSource {
    root = ../.;
    fileset = lib.fileset.unions [
      ../Cargo.toml
      ../Cargo.lock
      ../src
      ../assets
    ];
  };

  cargoLock = {
    lockFile = ../Cargo.lock;
    # Rune is a git dependency pinned by commit in Cargo.lock.
    allowBuiltinFetchGit = true;
  };

  nativeBuildInputs = [
    pkg-config
    makeWrapper
  ];

  buildInputs = [
    fontconfig
    libxkbcommon
    wayland
  ];

  # The sync tests drive real git against bare repositories in a temp dir.
  nativeCheckInputs = [ git ];
  preCheck = ''
    export HOME=$(mktemp -d)
    git config --global user.name nix
    git config --global user.email nix@localhost
    git config --global init.defaultBranch main
  '';

  postFixup = ''
    # mdwriter shells out to git (over SSH), gh and curl.
    wrapProgram $out/bin/mdwriter \
      --prefix PATH : ${lib.makeBinPath [ git gh curl openssh ]} \
      --prefix LD_LIBRARY_PATH : ${lib.makeLibraryPath runtimeLibs}
  '';

  meta = {
    description = "Write Markdown posts for a git-based site, with each post sent as its own pull request";
    homepage = "https://github.com/richard-parayno/mdwriter";
    license = lib.licenses.mit;
    mainProgram = "mdwriter";
    platforms = [ "x86_64-linux" ];
  };
}
