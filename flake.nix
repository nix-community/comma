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

  outputs = { self, nixpkgs, utils, naersk, flake-compat }:
    let
      inherit (nixpkgs) lib;
      commaLambda = pkgs:
        let
          naersk-lib = pkgs.callPackage naersk { };
        in
        naersk-lib.buildPackage {
          pname = "comma";
          root = ./.;
          nativeBuildInputs = with pkgs; [ makeWrapper ];
          overrideMain = _: {
            postInstall = ''
              wrapProgram $out/bin/comma \
                --prefix PATH : ${lib.makeBinPath (with pkgs; [ nix fzy nix-index-unwrapped ])}
              ln -s $out/bin/comma $out/bin/,

              mkdir -p $out/etc/profile.d
              cp $src/src/command-not-found.sh $out/etc/profile.d
              patchShebangs $out/etc/profile.d/command-not-found.sh
              sed -i "s|comma \"\$@\"|$out\/bin\/comma \"\$@\"|" $out/etc/profile.d/command-not-found.sh
            '';
          };
          checkInputs = [ pkgs.rustPackages.clippy ];
          doCheck = true;
          cargoTestCommands = x:
            x ++ [
              ''cargo clippy --all --all-features --tests -- \
                -D warnings || true''
            ];
        };
    in
    utils.lib.eachDefaultSystem
      (system:
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

          devShells.default = with pkgs; mkShell {
            nativeBuildInputs = [ cargo cargo-edit nix-index rustc rustfmt rustPackages.clippy fzy ];
            RUST_SRC_PATH = rustPlatform.rustLibSrc;
          };
        })
    // {
      overlays.default = (final: prev: {
        comma = commaLambda prev;
      });
    };
}
