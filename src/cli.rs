use clap::{crate_version, Args, Parser, Subcommand, ValueHint};
use clap_complete::Shell;

use crate::tools;

/// Runs programs without installing them
#[derive(Parser)]
#[clap(version = crate_version!())]
#[command(subcommand_negates_reqs = true)]
pub struct Opt {
    /// Generate the man page, then exit
    #[clap(long, hide = true)]
    pub mangen: bool,

    /// Install the derivation containing the executable into your profile,
    /// with `nix profile` if the profile is managed by it, otherwise with
    /// `nix-env`
    #[clap(short, long)]
    pub install: bool,

    /// Open a shell containing the derivations containing the executables.
    /// Every argument is an executable, e.g. `, -s cowsay lolcat`
    #[clap(short, long, conflicts_with_all = ["install", "print_packages", "print_path"])]
    pub shell: bool,

    /// Picker to use
    #[clap(short = 'P', long, env = "COMMA_PICKER", default_value = tools::FZY)]
    pub picker: String,

    /// Nixpkgs flake to use, e.g. `github:nixos/nixpkgs/nixos-unstable` [default: nixpkgs].
    /// When given, it takes precedence over `<nixpkgs>` from `NIX_PATH`
    #[clap(short = 'F', long, env = "COMMA_NIXPKGS_FLAKE")]
    pub nixpkgs_flake: Option<String>,

    /// Ask to confirm the program that will be run, before its package is
    /// downloaded. Not asked right after choosing the package in the picker.
    /// The question is asked on the terminal; without one the program isn't
    /// run. When it isn't run, comma exits with status 127
    #[clap(short, long, env = "COMMA_ASK_TO_CONFIRM")]
    pub ask: bool,

    /// Print completions for the given shell and exit
    #[clap(short = 'c', long = "print-completions")]
    pub print_completions: Option<Shell>,

    /// Print the package containing the executable
    #[clap(short = 'p', long = "print-packages")]
    pub print_packages: bool,

    /// Print the absolute path to the executable in the nix store
    #[clap(short = 'x', long = "print-path")]
    pub print_path: bool,

    /// Configure the cache level. 0 disables the cache, 1 enables cache for
    /// choices, 2 also caches path evaluations. Commands listed in the
    /// `COMMA_NO_CACHE_PATH` environment variable (comma separated) are treated
    /// as level 1
    #[clap(long = "cache-level", env = "COMMA_CACHING", default_value_t = 2)]
    pub cache_level: u8,

    /// Empty the cache
    #[clap(short, long = "empty-cache")]
    pub empty_cache: bool,

    /// Print the cached package choices and paths, then exit
    #[clap(long = "print-cache")]
    pub print_cache: bool,

    /// Overwrite the cache entry for the specified command. This is achieved
    /// by first deleting it from the cache, then running comma as normal
    #[clap(short, long = "delete-entry")]
    pub delete_entry: bool,

    /// Command to run, followed by its arguments. With `--shell`, the
    /// commands whose packages go in the shell
    #[clap(required_unless_present_any = ["empty_cache", "mangen", "print_completions", "print_cache"], name = "cmd", value_hint = ValueHint::Other, trailing_var_arg = true)]
    pub cmd: Vec<String>,

    #[clap(subcommand)]
    pub subcmds: Option<SubCmds>,
}

#[derive(Subcommand)]
#[clap(disable_help_subcommand = true)]
pub enum SubCmds {
    /// Show the manpage if it exists instead of running the executable
    ///
    /// Currently only supports Section 1 pages for programs.
    Man(ManArgs),
}

#[derive(Args)]
pub struct ManArgs {
    /// Command to show manpage for
    #[clap(required = true, name = "cmd", trailing_var_arg = true)]
    pub cmd: Vec<String>,
}
