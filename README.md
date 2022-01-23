# ,

Comma runs software without installing it.

Basically it just wraps together `nix run` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

```bash
nix-env -i -f .
```

### With nix flakes

The simplest way to use this as a nix flake is to do `nix run github:nix-community/comma -- <your command>`.  This is a little verbose, but you can create a convenient shell alias as follows:

```bash
alias ,="nix run github:nix-community/comma --"

# or if you want to override comma's nixpkgs input, do the following instead
# (replacing `nixpkgs` with whichever nixpkgs-providing flake you want)
alias ,="nix run github:nix-community/comma --inputs-from nixpkgs --"
```

Then you can invoke your command as `, <your command>`

If you want to make comma available slightly more declaratively, two flake attributes are provided: `overlay` and `defaultPackage.${system}`.

You can add it via `overlay` (preferred) as follows:

```nix
{
  inputs.comma.url = "github:nix-community/comma";

  outputs = { self, nixpkgs, comma }:
    let
      system = "x86_64-linux"; # replace this with your system arch
      overlays = [ comma.overlay ]
      pkgs = import nixpkgs { inherit system overlays; };
    in
    {
      devShell.${system} = pkgs.mkShell {
        name = "cool-stuff";
        buildInputs = [
          pkgs.comma # injected via overlay
          # ...
        ];
      };
    };
}
```

You can add it via `defaultPackage.${system}` as follows:

```nix
{
  inputs.comma.url = "github:nix-community/comma";

  # not mandatory but highly recommended
  # if not provided, it will use the nixpkgs referenced by this repo (https://github.com/nix-community/comma)
  inputs.comma.inputs.nixpkgs.follows = "nixpkgs";

  outputs = { self, nixpkgs, comma }:
    let
      system = "x86_64-linux"; # replace this with your system arch
      pkgs = import nixpkgs { inherit system; };
    in
    {
      devShell.${system} = pkgs.mkShell {
        name = "cool-stuff";
        buildInputs = [
          comma.defaultPackage.${system} # or comma.packages.${system}.comma
          # ...
        ];
      };
    };
}
```

## Usage

[See a quick demo on
YouTube](https://www.youtube.com/watch?v=VUM3Km_4gUg&list=PLRGI9KQ3_HP_OFRG6R-p4iFgMSK1t5BHs)

```bash
, cowsay neato
```
