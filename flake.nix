{
  description = "Comma runs software without installing it";

  inputs = {
    nixpkgs.url = "nixpkgs/nixos-unstable";
  };

  outputs = { self, nixpkgs, }:
  let
    b = builtins;
    lib = nixpkgs.lib;
    supportedSystems = [ "x86_64-linux" "x86_64-darwin" ];
    forAllSystems = f: lib.genAttrs supportedSystems
      (system: f system (import nixpkgs { inherit system; }));
  in
  rec {

    packages = forAllSystems
      (system: pkgs: {
        comma = import ./default.nix {
          inherit pkgs;
          updateScript = apps."${system}".update-index.program;
        };
      });

    defaultPackage = forAllSystems (system: pkgs: packages."${system}".comma);

    apps = forAllSystems
      (system: pkgs: {
        update-index = {
          type = "app";
          program = b.toString (pkgs.callPackage ./update-index.nix {});
        };
      });
  };
}
