{ pkgs
, stdenv
, lib
, fetchurl
, nix-index
, nix
, fzy
, makeWrapper
, runCommand

  # We use this to add matchers for stuff that's not in upstream nixpkgs, but is
  # in our own overlay. No fuzzy matching from multiple options here, it's just:
  # Was the command `, mything`? Run `nixpkgs.mything`.
, overlayPackages
}:

let

  # nix-index takes a little while to run and the contents don't change
  # meaningfully very often.
  indexCache = fetchurl {
    url = "https://github.com/Mic92/nix-index-database/releases/download/2021-12-12/index-x86_64-linux";
    sha256 = "sha256-+SoG5Qz2KWA/nIWXE6SLpdi8MDqTs8LY90fGZxGKOiA=";
  };

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
      --prefix PATH : ${nix-index}/bin \
      --prefix PATH : ${nix}/bin \
      --prefix PATH : ${fzy}/bin

    ln -s $out/bin/, $out/bin/comma
  '';
}
