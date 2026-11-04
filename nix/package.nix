{
  lib,
  rustPlatform,
  fetchPnpmDeps,
  pnpmConfigHook,
  pnpm_12,
  nodejs,
  cargo-tauri,
  pkg-config,
  wrapGAppsHook3,
  glib-networking,
  gtk3,
  libsoup_3,
  openssl,
  webkitgtk_4_1,
  git,
  gh,
  curl,
  openssh,
}:

let
  root = ../.;
in
rustPlatform.buildRustPackage (finalAttrs: {
  pname = "mdwriter";
  version = (lib.importJSON ../src-tauri/tauri.conf.json).version;

  src = lib.fileset.toSource {
    inherit root;
    fileset = lib.fileset.unions [
      ../index.html
      ../package.json
      ../pnpm-lock.yaml
      ../pnpm-workspace.yaml
      ../tsconfig.json
      ../vite.config.ts
      ../src
      (lib.fileset.difference ../src-tauri (lib.fileset.maybeMissing ../src-tauri/target))
    ];
  };

  cargoRoot = "src-tauri";
  buildAndTestSubdir = finalAttrs.cargoRoot;
  cargoLock.lockFile = ../src-tauri/Cargo.lock;

  pnpmDeps = fetchPnpmDeps {
    inherit (finalAttrs) pname version src;
    pnpm = pnpm_12;
    fetcherVersion = 5;
    hash = "sha256-eKINfkjfXurAYEuCLPz1v8JUe+e/fr6VvMPbfkR6H4w=";
  };

  nativeBuildInputs = [
    cargo-tauri.hook
    nodejs
    pnpm_12
    pnpmConfigHook
    pkg-config
    wrapGAppsHook3
  ];

  buildInputs = [
    glib-networking
    gtk3
    libsoup_3
    openssl
    webkitgtk_4_1
  ];

  # The sync tests drive real git against bare repositories in a temp dir.
  nativeCheckInputs = [ git ];
  preCheck = ''
    export HOME=$(mktemp -d)
    git config --global user.name nix
    git config --global user.email nix@localhost
    git config --global init.defaultBranch main
  '';
  cargoTestFlags = [ "--lib" ];

  preFixup = ''
    # mdwriter shells out to git (over SSH), gh and curl.
    gappsWrapperArgs+=(
      --prefix PATH : ${lib.makeBinPath [ git gh curl openssh ]}
      --set-default WEBKIT_DISABLE_DMABUF_RENDERER 1
    )
  '';

  meta = {
    description = "Write Markdown posts for a git-based site, with each post sent as its own pull request";
    homepage = "https://github.com/richard-parayno/mdwriter";
    mainProgram = "mdwriter";
    platforms = [ "x86_64-linux" ];
  };
})
