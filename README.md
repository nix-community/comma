# comma

Comma runs software without installing it.

Basically it just wraps together `nix shell -c` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

- Nix with [Flakes](https://nixos.wiki/wiki/Flakes):

  ```bash
  $ nix profile install github:nix-community/comma
  ```

- No flakes:

  ```bash
  $ nix-env -i -f "https://github.com/nix-community/comma/archive/master.tar.gz"
  ```

## Usage

```bash
, cowsay neato
```

## Prebuilt index

https://github.com/Mic92/nix-index-database
