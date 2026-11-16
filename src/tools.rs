//! The programs that comma runs. Packages replace these names with the store
//! paths of the programs, like `postPatch` in `flake.nix` does, so that they
//! don't have to be in PATH. Every program is named here and nowhere else, so
//! that this is the only file that has to be patched

pub const NIX: &str = "nix";
pub const NIX_ENV: &str = "nix-env";
pub const NIX_LOCATE: &str = "nix-locate";
pub const FZY: &str = "fzy";
