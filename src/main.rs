mod cache;
mod index;
mod shell;

use std::{
    collections::HashMap,
    env,
    io::{self, Write},
    os::unix::prelude::CommandExt,
    path::Path,
    process::{self, Command, ExitCode, Stdio},
};

use cache::{Cache, CacheEntry};
use clap::{crate_version, Args, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{generate, Generator, Shell};
use log::{debug, error, trace};

fn pick(picker: &str, derivations: &[String]) -> Option<String> {
    let mut picker_process = Command::new(picker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to execute {picker}: {err}"));

    let picker_stdin = picker_process.stdin.as_mut().unwrap();

    picker_stdin
        .write_all(derivations.join("\n").as_bytes())
        .expect("failure to write stdin");

    let output = picker_process.wait_with_output().unwrap().stdout;

    if output.is_empty() {
        return None;
    }
    Some(
        std::str::from_utf8(&output)
            .unwrap_or_else(|e| panic!("{picker} outputted invalid UTF-8: {e}"))
            .trim()
            .to_owned(),
    )
}

fn index_database(command: &str) -> Option<Box<[String]>> {
    index::check_database_updated();

    let nix_locate_output = Command::new("nix-locate")
        .args(["--minimal", "--at-root", "--whole-name"])
        .arg(format!("/bin/{command}"))
        .output()
        .expect("failed to execute nix-locate");

    if !nix_locate_output.status.success() {
        match std::str::from_utf8(&nix_locate_output.stderr) {
            Ok(stderr) => eprintln!("nix-locate failed with: {stderr}"),
            Err(_) => eprintln!("nix-locate failed"),
        }
        return None;
    }

    let attrs = nix_locate_output.stdout;

    if attrs.is_empty() {
        eprintln!("No executable `{command}` found in nix-index database.");
        return None;
    }

    Some(
        std::str::from_utf8(&attrs)
            .expect("fail")
            .trim()
            .split('\n')
            .map(|s| s.to_owned())
            .collect(),
    )
}

fn index_database_pick(command: &str, picker: &str) -> Option<String> {
    let attrs = index_database(command)?;

    if attrs.len() > 1 {
        pick(picker, &attrs)
    } else {
        attrs.first().map(|s| s.trim().to_owned())
    }
}

fn run_command_or_open_shell(
    use_channel: bool,
    choice: &str,
    command: &str,
    trail: &[&str],
    nixpkgs_flake: &str,
) -> Command {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "shell",
    ]);

    if use_channel {
        run_cmd.args(["-f", "<nixpkgs>", choice]);
    } else {
        run_cmd.args([format!("{nixpkgs_flake}#{choice}")]);
    }

    if !command.is_empty() {
        run_cmd.args(["--command", command]);
        if !trail.is_empty() {
            run_cmd.args(trail);
        }
    };

    trace!("run nix command arguments: {run_cmd:?}");

    run_cmd
}

fn get_command_path(
    use_channel: bool,
    choice: &str,
    command: &str,
    nixpkgs_flake: &str,
) -> Option<String> {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "build",
        "--print-out-paths",
        "--no-link",
    ]);

    if use_channel {
        run_cmd.args(["-f", "<nixpkgs>", choice]);
    } else {
        run_cmd.args([format!("{nixpkgs_flake}#{choice}")]);
    }

    let result = run_cmd
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to execute nix: {err}"));

    // It is safe to assume that only one path will be printed because
    // nix-locate appends the output to the derivation name
    // (e.g., firefox.out instead of firefox)
    let output = result.wait_with_output().unwrap();
    if !output.status.success() {
        error!("nix build failed for '{choice}'");
        return None;
    }
    let base_path = std::str::from_utf8(&output.stdout)
        .unwrap_or_else(|err| panic!("nix outputted invalid UTF-8: {err}"))
        .trim();

    // It is safe to assume that command is in $out/bin/{command} from
    // the derivation, since this was already filtered by nix-locate
    Some(format!("{base_path}/bin/{command}"))
}

/// Find the path of `command`. Paths are only read from and written to `cache`
/// when it is given
fn get_command_path_from_cache(
    cache: Option<&mut Cache>,
    entry: &CacheEntry,
    use_channel: bool,
    command: &str,
    nixpkgs_flake: &str,
) -> Option<String> {
    // The nixpkgs the path is built from, used as the key for cached paths.
    // <nixpkgs> is resolved through NIX_PATH, so include it in the key
    let source = if use_channel {
        format!(
            "<nixpkgs> (NIX_PATH={})",
            env::var("NIX_PATH").unwrap_or_default()
        )
    } else {
        nixpkgs_flake.to_owned()
    };
    let source = source.as_str();

    let cached = cache.as_ref().and(entry.paths.get(source));

    match cached {
        // If we have the path for this nixpkgs source in the cache and it is
        // not garbage collected (so the path still exists), it should be safe
        // to use it directly
        Some(path) if Path::new(path).exists() => {
            debug!("found path from cache for command '{command}' using '{source}': {path}");
            Some(path.to_owned())
        }
        // Otherwise, we need to find the command path
        _ => {
            let path = get_command_path(use_channel, &entry.derivation, command, nixpkgs_flake)?;
            debug!("found path from nix for command '{command}' using '{source}': {path}");

            if let Some(cache) = cache {
                let mut entry = entry.clone();
                entry.paths.insert(source.to_owned(), path.clone());
                cache.update(command, entry);
            }

            Some(path)
        }
    }
}

fn run_command_from_cache(
    cache: Option<&mut Cache>,
    entry: &CacheEntry,
    use_channel: bool,
    command: &str,
    trail: &[String],
    nixpkgs_flake: &str,
) -> Option<Command> {
    let path = get_command_path_from_cache(cache, entry, use_channel, command, nixpkgs_flake)?;

    let mut run_cmd = Command::new(path);
    if !trail.is_empty() {
        run_cmd.args(trail);
    }

    trace!("run command from cache arguments: {run_cmd:?}");

    Some(run_cmd)
}

fn confirmer(run_cmd: &Command) -> bool {
    loop {
        print!("Run '{}'? [Y/n]: ", run_cmd.get_program().display());
        io::stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin().read_line(&mut input).unwrap();

        match input.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => return true,
            "n" | "no" => return false,
            _ => {
                println!("Please enter 'y' or 'n'.");
            }
        }
    }
}

fn main() -> ExitCode {
    env_logger::init();

    let args = Opt::parse();

    if args.mangen {
        use clap::CommandFactory;
        let man = clap_mangen::Man::new(Opt::command());

        if let Err(err) = man.render(&mut std::io::stdout()) {
            panic!("{}", err)
        } else {
            return ExitCode::SUCCESS;
        }
    }

    if let Some(shell) = args.print_completions {
        let mut cmd = Opt::command();
        eprintln!("Generating completion file for {shell}...");
        let bin_name = std::env::args()
            .next()
            .and_then(|p| Path::new(&p).file_name().map(|f| f.to_string_lossy().into_owned()))
            .unwrap_or_else(|| "comma".into());
        print_completions(shell, &mut cmd, &bin_name);
        return ExitCode::SUCCESS;
    }

    let mut cache = if args.cache_level == 0 {
        None
    } else {
        match Cache::new() {
            Err(e) => {
                error!("failed to initialize cache, disabling related functionality: {e}");
                None
            }
            Ok(x) => Some(x),
        }
    };

    if args.empty_cache {
        if let Some(ref mut cache) = cache {
            cache.empty();
        }
    }

    if args.print_cache {
        return match cache {
            Some(ref cache) => {
                print_cache(cache);
                ExitCode::SUCCESS
            }
            None => {
                eprintln!("The cache is disabled");
                ExitCode::FAILURE
            }
        };
    }

    if args.cmd.is_empty() && args.subcmds.is_none() {
        return if args.empty_cache {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let (command, trail) = if let Some(SubCmds::Man(ManArgs { ref cmd })) = args.subcmds {
        (&cmd[0], &cmd[1..])
    } else {
        (&args.cmd[0], &args.cmd[1..])
    };

    if args.delete_entry {
        if let Some(ref mut cache) = cache {
            cache.delete(command);
        }
    }

    if args.print_packages {
        match index_database(command) {
            Some(derivations) => {
                println!(
                    "Packages that contain /bin/{command}:\n{}",
                    derivations
                        .iter()
                        .map(|a| format!("- {a}"))
                        .collect::<Box<[String]>>()
                        .join("\n")
                );

                return ExitCode::SUCCESS;
            }
            None => return ExitCode::FAILURE,
        }
    }

    let entry = match cache {
        Some(ref mut cache) => cache.query(command).or_else(|| {
            index_database_pick(command, &args.picker).map(|derivation| {
                let entry = CacheEntry {
                    derivation,
                    paths: HashMap::new(),
                };
                cache.update(command, entry.clone());
                entry
            })
        }),
        None => index_database_pick(command, &args.picker).map(|derivation| CacheEntry {
            derivation,
            paths: HashMap::new(),
        }),
    };

    let Some(entry) = entry else {
        return ExitCode::FAILURE;
    };

    // Whether evaluated paths for this command are read from and written to
    // the cache. Commands listed in COMMA_NO_CACHE_PATH (comma separated) are
    // always evaluated again, but their package choice is still cached
    let no_cache_path = env::var("COMMA_NO_CACHE_PATH").unwrap_or_default();
    let cache_paths =
        args.cache_level >= 2 && !no_cache_path.split(',').any(|c| c.trim() == command);
    if !cache_paths {
        debug!(
            "cache_level={}, COMMA_NO_CACHE_PATH={no_cache_path:?}: not using cached paths for '{command}'",
            args.cache_level
        );
    }

    let basename = entry.derivation.rsplit('.').next_back().unwrap();

    // The cache used for path lookups; the choice is still kept in `cache`
    let path_cache = cache.as_mut().filter(|_| cache_paths);

    // An explicitly given flake (via -F or COMMA_NIXPKGS_FLAKE) takes
    // precedence over <nixpkgs> from NIX_PATH. An empty value counts as not
    // given, so `COMMA_NIXPKGS_FLAKE=` doesn't turn into the flake in the
    // current directory
    let explicit_flake = args.nixpkgs_flake.as_deref().filter(|s| !s.is_empty());
    let use_channel = explicit_flake.is_none()
        && env::var("NIX_PATH")
            .unwrap_or_default()
            .contains("nixpkgs=");
    let nixpkgs_flake = explicit_flake.unwrap_or("nixpkgs");

    if args.install {
        let _ = Command::new("nix-env")
            .args(["-f", "<nixpkgs>", "-iA", basename])
            .exec();
    } else if args.shell {
        // TODO: use cache here, but this is tricky since it actually depends in `nix-shell`
        let shell_cmd = shell::select_shell_from_pid(process::id()).unwrap_or("bash".into());
        let _ = run_command_or_open_shell(
            use_channel,
            &entry.derivation,
            &shell_cmd,
            &[],
            nixpkgs_flake,
        )
        .exec();
    } else if args.print_path {
        let Some(path) =
            get_command_path_from_cache(path_cache, &entry, use_channel, command, nixpkgs_flake)
        else {
            return ExitCode::FAILURE;
        };
        println!("{path}");
    } else if args.subcmds.is_some() {
        // Open manpage via
        // nix shell nixpkgs#drvName --command man commandName
        let err = run_command_or_open_shell(
            use_channel,
            &entry.derivation.replace(".out", "^*"),
            "man",
            &[command],
            nixpkgs_flake,
        )
        .exec();

        // This code will only run if an error occurs launching
        eprintln!("{err:?}");
        return ExitCode::FAILURE;
    } else {
        let Some(mut run_cmd) = run_command_from_cache(
            path_cache,
            &entry,
            use_channel,
            command,
            trail,
            nixpkgs_flake,
        ) else {
            return ExitCode::FAILURE;
        };

        // Drop cache before calling exec() to make sure that
        // the cache file is written
        drop(cache);
        let answer = if args.ask { confirmer(&run_cmd) } else { true };
        if answer {
            let _ = run_cmd.exec();
        }
    }

    ExitCode::SUCCESS
}

fn print_cache(cache: &Cache) {
    for (command, entry) in cache.entries() {
        println!("{command} -> {}", entry.derivation);

        let mut paths: Vec<_> = entry.paths.iter().collect();
        paths.sort();
        let width = paths
            .iter()
            .map(|(source, _)| source.len())
            .max()
            .unwrap_or(0);
        for (source, path) in paths {
            let note = if Path::new(path).exists() {
                ""
            } else {
                " (garbage collected)"
            };
            println!("  {source:width$}  {path}{note}");
        }
    }
}

fn print_completions<G: Generator>(generator: G, cmd: &mut clap::Command, bin_name: &str) {
    generate(
        generator,
        cmd,
        bin_name,
        &mut io::stdout(),
    );
}

/// Runs programs without installing them
#[derive(Parser)]
#[clap(version = crate_version!())]
#[command(subcommand_negates_reqs = true)]
struct Opt {
    /// Generate the man page, then exit
    #[clap(long, hide = true)]
    mangen: bool,

    /// Install the derivation containing the executable
    #[clap(short, long)]
    install: bool,

    /// Open a shell containing the derivation containing the executable
    #[clap(short, long)]
    shell: bool,

    /// Picker to use
    #[clap(short = 'P', long, env = "COMMA_PICKER", default_value = "fzy")]
    picker: String,

    /// Nixpkgs flake to use, e.g. `nixpkgs/nixos-unstable` [default: nixpkgs].
    /// When given, it takes precedence over `<nixpkgs>` from NIX_PATH
    #[clap(short = 'F', long, env = "COMMA_NIXPKGS_FLAKE")]
    nixpkgs_flake: Option<String>,

    /// Ask to confirm the program that will be run.
    #[clap(short, long, env = "COMMA_ASK_TO_CONFIRM")]
    ask: bool,

    /// Print completions for the given shell and exit
    #[clap(short = 'c', long = "print-completions")]
    print_completions: Option<Shell>,

    /// Print the package containing the executable
    #[clap(short = 'p', long = "print-packages")]
    print_packages: bool,

    /// Print the absolute path to the executable in the nix store
    #[clap(short = 'x', long = "print-path")]
    print_path: bool,

    /// Configure the cache level. 0 disables the cache, 1 enables cache for
    /// choices, 2 also caches path evaluations. Commands listed in the
    /// COMMA_NO_CACHE_PATH environment variable (comma separated) are treated
    /// as level 1
    #[clap(long = "cache-level", env = "COMMA_CACHING", default_value_t = 2)]
    cache_level: u8,

    /// Empty the cache
    #[clap(short, long = "empty-cache")]
    empty_cache: bool,

    /// Print the cached package choices and paths, then exit
    #[clap(long = "print-cache")]
    print_cache: bool,

    /// Overwrite the cache entry for the specified command. This is achieved
    /// by first deleting it from the cache, then running comma as normal
    #[clap(short, long = "delete-entry")]
    delete_entry: bool,

    /// Command to run
    #[clap(required_unless_present_any = ["empty_cache", "mangen", "print_completions", "print_cache"], name = "cmd", value_hint = ValueHint::Other, trailing_var_arg = true)]
    cmd: Vec<String>,

    #[clap(subcommand)]
    subcmds: Option<SubCmds>,
}

#[derive(Subcommand)]
#[clap(disable_help_subcommand = true)]
enum SubCmds {
    /// Show the manpage if it exists instead of running the executable
    ///
    /// Currently only supports Section 1 pages for programs.
    Man(ManArgs),
}

#[derive(Args)]
struct ManArgs {
    /// Command to show manpage for
    #[clap(required = true, name = "cmd", trailing_var_arg = true)]
    cmd: Vec<String>,
}
