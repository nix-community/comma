{
  description = "Comma runs software without installing it";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/21.11";
  inputs.flake-utils.url = "github:numtide/flake-utils";

  outputs = { self, nixpkgs, flake-utils }:
    let
      mkComma = pkgs: import ./default.nix { inherit pkgs; };
    in
    flake-utils.lib.eachDefaultSystem
      (
        system:
        let
          pkgs = import nixpkgs { inherit system; };
        in
        rec {
          defaultPackage = packages.comma;
          packages = flake-utils.lib.flattenTree {
            comma = mkComma pkgs;
          };
        }
      ) // {
      overlay = final: prev: {
        comma = mkComma final;
      };
    };
}
