# ,

Comma runs software without installing it.

Basically it just wraps together `nix run` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

```bash
nix-env -i -f .
```

## Usage

```bash
, cowsay neato
```
