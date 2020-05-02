# Comma: the laconic Nix toolkit

`,` runs software without installing it.

`,i` installs that software to your `nix-env` profile.

`,q` has a suite of subcommands for interacting with the Nix Store:

```
,ql  <regex>       : (list nix-store entries matching regex)
,qs  <store-path*> : nix show-derivation

,qo  <store-path*> : nix-store --query --outputs
,qd  <store-path*> : nix-store --query --deriver

,q-  <store-path*> : nix-store --query --references
,q-- <store-path*> : nix-store --query --requisites
,q+  <store-path*> : nix-store --query --referers
,q++ <store-path*> : nix-store --query --referers-closure

,qx  <store-path*> : nix-store --realise

Anything taking <store-path*> can also be called like:
  echo <store-path*> | ,q-
```

Basically it just wraps together `nix run` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

```bash
nix-env -i -f .
```

## `,` Usage

[See a quick demo of `,` on
YouTube](https://www.youtube.com/watch?v=VUM3Km_4gUg&list=PLRGI9KQ3_HP_OFRG6R-p4iFgMSK1t5BHs)

```bash
, cowsay neato
,i ripgrep
```

## `,q` Usage

```
,q-- ~/.nix-profile | grep ruby | ,qd | ,qo | grep devdoc | ,qx
```

This is equivalent to something like:

```
rubies=$(nix-store -qR ~/.nix-profile | grep ruby)
docs=$(nix-store -q --outputs $(nix-store -qd $rubies) | grep devdoc)
nix-store --realise $docs
```
