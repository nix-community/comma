{ pkgs ? import <nixpkgs> { }

, stdenv ? pkgs.stdenv
, lib ? pkgs.lib
, fetchurl ? pkgs.fetchurl
, nix ? pkgs.nix
, fzy ? pkgs.fzy
, makeWrapper ? pkgs.makeWrapper

# We use this to add matchers for stuff that's not in upstream nixpkgs, but is
# in our own overlay. No fuzzy matching from multiple options here, it's just:
# Was the command `, mything`? Run `nixpkgs.mything`.
, overlayPackages ? []
}:

stdenv.mkDerivation rec {
  name = "comma";

  src = ./.;

  nativeBuildInputs = [ makeWrapper ];

  installPhase = let
    caseCondition = lib.concatStringsSep "|" (overlayPackages ++ [ "--placeholder--" ]);
  in ''
    mkdir -p $out/bin
    sed -e 's/@OVERLAY_PACKAGES@/${caseCondition}/' < , > $out/bin/,
    chmod +x $out/bin/,
    wrapProgram $out/bin/, \
      --prefix PATH : ${nix}/bin \
      --prefix PATH : ${fzy}/bin

    ln -s $out/bin/, $out/bin/comma
  '';
}
