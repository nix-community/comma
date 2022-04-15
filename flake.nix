{
  description = "Comma runs software without installing it";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, }:
  let
    b = builtins;
    lib = nixpkgs.lib;
    supportedSystems = lib.systems.supported.hydra;
    forAllSystems = f: lib.genAttrs supportedSystems
      (system: f system (import nixpkgs { inherit system; }));
  in
  rec {

    packages = forAllSystems
      (system: pkgs: {
        comma = import ./default.nix {
          inherit pkgs;
        };
      });

    defaultPackage = forAllSystems (system: pkgs: packages."${system}".comma);

  };
}
