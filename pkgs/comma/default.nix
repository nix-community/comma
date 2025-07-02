{
  lib,
  rustPlatform,
  makeWrapper,
  callPackage,
  clippy,
  nix,
  fzy,
  nix-index-unwrapped,
  # Flake inputs
  self,
}:

rustPlatform.buildRustPackage (finalAttrs: {
  pname = "comma";
  inherit ((lib.importTOML "${finalAttrs.src}/Cargo.toml").package) version;
  src = self;

  strictDeps = true;

  nativeBuildInputs = [ makeWrapper ];
  buildInputs = [
    nix
    fzy
    nix-index-unwrapped
  ];
  # TODO: This might not support cross-compiling
  nativeCheckInputs = [ clippy ] ++ finalAttrs.buildInputs;

  useFetchCargoVendor = true;
  cargoLock = {
    lockFile = "${finalAttrs.src}/Cargo.lock";
  };

  postCheck = ''
    cargo clippy --all --all-features --tests -- -D warnings
  '';

  postInstall = ''
    wrapProgram $out/bin/comma \
      --suffix PATH : ${
        lib.makeBinPath finalAttrs.buildInputs
      }
    ln -s $out/bin/comma $out/bin/,
  '';
})
