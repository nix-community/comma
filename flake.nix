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
      commaLambda =
        pkgs:
        pkgs.callPackage (
          {
            callPackage,
            makeWrapper,
            nix,
            fzy,
            nix-index-unwrapped,
            rustPackages,
          }:
          let
            naersk-lib = callPackage naersk { };
          in
          naersk-lib.buildPackage {
            pname = "comma";
            src = self;
            nativeBuildInputs = [ makeWrapper ];
            overrideMain = _: {
              postInstall = ''
                wrapProgram $out/bin/comma \
                  --prefix PATH : ${
                    lib.makeBinPath ([
                      nix
                      fzy
                      nix-index-unwrapped
                    ])
                  }
                ln -s $out/bin/comma $out/bin/,

                mkdir -p $out/etc/profile.d
                cp $src/src/comma-command-not-found.sh $out/etc/profile.d
                patchShebangs $out/etc/profile.d/comma-command-not-found.sh
                sed -i "s|comma --ask \"\$@\"|$out\/bin\/comma --ask \"\$@\"|" $out/etc/profile.d/comma-command-not-found.sh
              '';
            };
            checkInputs = [ rustPackages.clippy ];
            doCheck = true;
            cargoTestCommands =
              x:
              x
              ++ [
                ''
                  cargo clippy --all --all-features --tests -- \
                                  -D warnings || true''
              ];
          }
        ) { };
    in
    utils.lib.eachDefaultSystem (
      system:
      let
        pkgs = nixpkgs.legacyPackages.${system};
      in
      {
        packages = {
          default = self.packages."${system}".comma;
          comma = commaLambda pkgs;
        };

        apps.default = utils.lib.mkApp {
          drv = self.packages."${system}".default;
        };

        devShells.default =
          with pkgs;
          mkShell {
            nativeBuildInputs = [
              cargo
              cargo-edit
              nix-index
              rustc
              rustfmt
              rustPackages.clippy
              fzy
            ];
            RUST_SRC_PATH = rustPlatform.rustLibSrc;
          };

        formatter = pkgs.nixfmt-tree;
      }
    )
    // {
      overlays.default = (
        final: prev: {
          comma = commaLambda prev;
        }
      );
    };
}
