{
  description = "runs programs without installing them";

  inputs = {
    naersk = {
      url = "github:nix-community/naersk/master";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    nixpkgs.url = "github:NixOS/nixpkgs/nixpkgs-unstable";
    utils.url = "github:numtide/flake-utils";
    flake-compat = {
      url = "github:edolstra/flake-compat";
      flake = false;
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      utils,
      naersk,
      flake-compat,
    }:
    let
      inherit (nixpkgs) lib;
    in
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
        inherit (pkgs) callPackage mkShell rustPlatform;
      in
      {
        packages = {
          default = self.packages."${system}".comma;
          comma = callPackage "${self}/pkgs/comma" {
            inherit self naersk;
          };
        };

        apps.default = utils.lib.mkApp {
          drv = self.packages."${system}".default;
        };

        devShells.default = mkShell {
          nativeBuildInputs = with pkgs; [
            cargo
            cargo-edit
            nix-index
            rustc
            rustfmt
            clippy
            fzy
          ];
          RUST_SRC_PATH = rustPlatform.rustLibSrc;
        };
      }
    )
    // {
      overlays.default = (
        final: prev: {
          comma = prev.callPackage "${self}/pkgs/comma" {
            inherit self naersk;
          };
        }
      );
    };
}
