# ,

Comma runs software without installing it.

Basically it just wraps together `nix run` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

```bash
nix-env -i -f .
```

## Usage

[See a quick demo on
YouTube](https://www.youtube.com/watch?v=VUM3Km_4gUg&list=PLRGI9KQ3_HP_OFRG6R-p4iFgMSK1t5BHs)

```bash
, cowsay neato
```


## Prebuilt index

https://github.com/Mic92/nix-index-database
