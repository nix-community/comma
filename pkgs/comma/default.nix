{
  lib,
  makeWrapper,
  callPackage,
  clippy,
  nix,
  fzy,
  nix-index-unwrapped,
  # Flake inputs
  self,
  naersk,
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
    '';
  };

  doCheck = true;
  checkInputs = [ clippy ];
  cargoTestCommands = x: x ++ [ "cargo clippy --all --all-features --tests -- -D warnings || true" ];
}
