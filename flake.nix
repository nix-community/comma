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

  outputs = { self, nixpkgs, utils, naersk, flake-compat }: {
      overlays.default = final: prev:
        let
          naersk-lib = final.callPackage naersk { };
        in
        {
          comma = naersk-lib.buildPackage {
            pname = "comma";
            root = ./.;
            nativeBuildInputs = with final; [ makeWrapper ];
            overrideMain = _: {
              postInstall = ''
                wrapProgram $out/bin/comma \
                  --prefix PATH : ${final.lib.makeBinPath (with final; [ nix fzy nix-index-unwrapped ])}
                ln -s $out/bin/comma $out/bin/,
              '';
            };
          };
      };
    }
    //
    utils.lib.eachDefaultSystem (system:
      let
        inherit (nixpkgs) lib;
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ self.overlays.default ];
        };
      in
      {
        packages = {
          default = self.packages."${system}".comma;
          inherit (pkgs) comma;
        };

        apps.default = utils.lib.mkApp {
          drv = self.packages."${system}".default;
        };

        devShells.default = with pkgs; mkShell {
          nativeBuildInputs = [ cargo rustc rustfmt rustPackages.clippy fzy ];
          RUST_SRC_PATH = rustPlatform.rustLibSrc;
        };
      });
}
