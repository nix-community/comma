# comma

Comma runs software without installing it.

Basically it just wraps together `nix shell -c` and `nix-index`. You stick a `,` in front of a command to
run it from whatever location it happens to occupy in `nixpkgs` without really thinking about it.

## Installation

  comma is in nixpkgs so you can install it just like any other package.

  Either add `comma` to `systemPackages` in your NixOS configuration (PREFERRED)

  ```nix
  environment.systemPackages = with pkgs; [ comma ];
  ```

  Or install it in your nix environment. (DISCOURAGED)

  ```bash
  nix-env -f '<nixpkgs>' -iA comma
  ```

  Get the required nix-index database from

  [nix-index-database ad-hoc-download section](https://github.com/nix-community/nix-index-database?tab=readme-ov-file#ad-hoc-download)
  Remember to keep it up to date

  Alternatively you may use `programs.nix-index-database.comma.enable` in the module from [nix-index-database](https://github.com/nix-community/nix-index-database), in that case do not add `comma` to `systemPackages` yourself.

## Usage

```bash
, cowsay neato
```

### Cache

Comma supports caching both the choices (i.e., once you select a derivation for
a command, it will always return the same derivation) and paths (i.e., once the
path is evaluated by Nix, we will always return the same path until it is GC'd).
You can control those options by using `--cache-level` flag or `COMMA_CACHING`
environment variable:

- `0`: completely disables caching
- `1`: only cache choices
- `2` (default): also caches paths

Cache for path is the default since it makes subsequent usage of a command much
faster:

```
$ hyperfine "./result/bin/comma --cache-level=1 ls" "./result/bin/comma --cache-level=2 ls"
Benchmark 1: ./result/bin/comma --cache-level=1 ls
  Time (mean ± σ):      1.050 s ±  0.021 s    [User: 0.540 s, System: 0.210 s]
  Range (min … max):    1.009 s …  1.075 s    10 runs

Benchmark 2: ./result/bin/comma --cache-level=2 ls
  Time (mean ± σ):       6.6 ms ±   1.0 ms    [User: 3.0 ms, System: 3.5 ms]
  Range (min … max):     5.8 ms …  11.3 ms    297 runs

  Warning: Statistical outliers were detected. Consider re-running this benchmark on a quiet system without any interferences from other programs. It might help to use the '--warmup' or '--prepare' options.

Summary
  ./result/bin/comma --cache-level=2 ls ran
  159.25 ± 23.44 times faster than ./result/bin/comma --cache-level=1 ls
```

However, it also means you may not run the most up-to-date version of a
command, specially if you don't run Nix's garbage collector often. If this is
an issue for you, set `COMMA_CACHING=1`.

## Prebuilt index

https://github.com/Mic92/nix-index-database
