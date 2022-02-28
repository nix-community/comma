{ pkgs ? import <nixpkgs> { }

, stdenv ? pkgs.stdenv
, lib ? pkgs.lib
, fetchurl ? pkgs.fetchurl
, nix-index ? pkgs.nix-index
, nix ? pkgs.nix
, fzy ? pkgs.fzy
, makeWrapper ? pkgs.makeWrapper
, runCommand ? pkgs.runCommand
, updateScript ? import ./update-index.nix { inherit pkgs; }
, linkFarm ? pkgs.linkFarm

# We use this to add matchers for stuff that's not in upstream nixpkgs, but is
# in our own overlay. No fuzzy matching from multiple options here, it's just:
# Was the command `, mything`? Run `nixpkgs.mything`.
, overlayPackages ? []
}:

let
  indexCaches = {
    x86_64-linux = fetchurl {
      url = "https://github.com/Mic92/nix-index-database/releases/download/2021-12-12/index-x86_64-linux";
      hash = "sha256-+SoG5Qz2KWA/nIWXE6SLpdi8MDqTs8LY90fGZxGKOiA=";
    };

    x86_64-darwin = fetchurl {
      url = "https://github.com/Mic92/nix-index-database/releases/download/2022-02-27/index-x86_64-darwin";
      hash = "sha256-sHGUSjd6EOpzdWtS5FGtTkS9KEKvDCGMHTYVwxOkZIo=";
    };
  };

  # nix-index takes a little while to run and the contents don't change
  # meaningfully very often.
  indexCache = indexCaches.${stdenv.hostPlatform.system} or (throw "unsupported system: ${stdenv.hostPlatform.system}");

  # nix-locate needs the --db argument to be a directory containing a file
  # named "files".
  nixIndexDB = linkFarm "nix-index-cache" [
    { name = "files"; path = indexCache; }
  ];

in
stdenv.mkDerivation rec {
  name = "comma";

  src = ./.;

  buildInputs = [ nix-index nix fzy ];
  nativeBuildInputs = [ makeWrapper ];

  installPhase = let
    caseCondition = lib.concatStringsSep "|" (overlayPackages ++ [ "--placeholder--" ]);
  in ''
    mkdir -p $out/bin
    sed -e 's/@OVERLAY_PACKAGES@/${caseCondition}/' < , > $out/bin/,
    chmod +x $out/bin/,
    wrapProgram $out/bin/, \
      --set PREBUILT_NIX_INDEX_DB ${nixIndexDB} \
      --set NIXPKGS ${pkgs.path} \
      --set UPDATE_SCRIPT ${updateScript} \
      --prefix PATH : ${nix-index}/bin \
      --prefix PATH : ${nix}/bin \
      --prefix PATH : ${fzy}/bin

    ln -s $out/bin/, $out/bin/comma
  '';
}
