# ,

Comma runs software without installing it.

Basically it just wraps together `nix run` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

```bash
nix-env -i -f .
```

### With nix flakes (experimental)

Two flake attributes are provided: `overlay` and `defaultPackage.${system}`.

You can add it via `overlay` (preferred) as follows:

```nix
{
  inputs.comma.url = "github:Shopify/comma";

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
  inputs.comma.url = "github:Shopify/comma";

  # not mandatory but highly recommended
  # if not provided, it will use the nixpkgs referenced by this repo (https://github.com/Shopify/comma)
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
