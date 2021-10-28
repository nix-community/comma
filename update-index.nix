{
  pkgs ? import <nixpkgs> {},

  coreutils ? pkgs.coreutils,
  gnugrep ? pkgs.gnugrep,
  lib ? pkgs.lib,
  nix-index ? pkgs.nix-index,
  writeScript ? pkgs.writeScript,
}:

writeScript "update-index" ''
  PATH=${lib.makeBinPath [
    coreutils
    gnugrep
    nix-index
  ]}

  # on flake based installations nixpkgs is specified via
  # flake input and therefore NIX_PATH might be unset
  if echo $NIX_PATH | grep -q "nixpkgs="; then
    nixpkgs=""
  else
    nixpkgs="-I nixpkgs=${pkgs.path}"
  fi

  mkdir -p $HOME/.cache/comma/
  nix-index -d $HOME/.cache/nix-index -f $nixpkgs
''
