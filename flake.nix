{
  description = "mdwriter: write Markdown posts for a git-based site, with each post sent as its own pull request";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs =
    { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAll = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAll (pkgs: {
        default = pkgs.callPackage ./nix/package.nix { };
      });

      # For a NixOS configuration: `nixpkgs.overlays = [ mdwriter.overlays.default ];`, then
      # `environment.systemPackages = [ pkgs.mdwriter ];`.
      overlays.default = final: _prev: {
        mdwriter = final.callPackage ./nix/package.nix { };
      };
    };
}
