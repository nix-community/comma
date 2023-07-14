# comma

Comma runs software without installing it.

Basically it just wraps together `nix shell -c` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

  comma is in nixpkgs so you can install it just like any other package.

  either install it in your nix environment

  ```bash
    nix-env -f '<nixpkgs>' -iA comma
  ```

  or add this snippet to your NixOS configuration.

  ```nix
  environment.systemPackages = with pkgs; [ comma ];
  ```

## Usage

```bash
, cowsay neato
```

## Prebuilt index

https://github.com/Mic92/nix-index-database
