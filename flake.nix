{
  description = "Comma runs software without installing it";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/21.05";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    flake-utils.lib.eachDefaultSystem
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        rec {
          defaultPackage = packages.comma;
          packages = flake-utils.lib.flattenTree {
            comma = import ./default.nix { inherit pkgs; };
          };
        }
      ) // {
      overlay = final: prev: {
        comma = import ./default.nix { pkgs = final; };
      };
    };
}
