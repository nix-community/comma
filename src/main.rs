mod cache;
mod index;
mod shell;

use std::{
    collections::HashMap,
    env,
    fs::OpenOptions,
    io::{self, BufRead, BufReader, Write},
    os::unix::prelude::CommandExt,
    path::Path,
    process::{self, Command, ExitCode, Stdio},
};

use cache::{Cache, CacheEntry};
use clap::{crate_version, Args, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{generate, Generator, Shell};
use log::{debug, error, trace};

/// The nixpkgs that packages are built from
#[derive(Debug, PartialEq)]
enum Nixpkgs {
    /// `<nixpkgs>`, which nix resolves through the given `NIX_PATH`
    Channel { nix_path: String },
    /// A flake reference, e.g. `nixpkgs` or `github:nixos/nixpkgs/nixos-unstable`
    Flake(String),
}

impl Nixpkgs {
    /// An explicitly given flake (via -F or `COMMA_NIXPKGS_FLAKE`) takes
    /// precedence over `<nixpkgs>` from `NIX_PATH`
    fn from_args(explicit_flake: Option<&str>) -> Self {
        Self::resolve(explicit_flake, env::var("NIX_PATH").unwrap_or_default())
    }

    fn resolve(explicit_flake: Option<&str>, nix_path: String) -> Self {
        // An empty value counts as not given, so `COMMA_NIXPKGS_FLAKE=`
        // doesn't turn into the flake in the current directory
        match explicit_flake.filter(|s| !s.is_empty()) {
            Some(flake) => Self::Flake(flake.to_owned()),
            None if nix_path.contains("nixpkgs=") => Self::Channel { nix_path },
            None => Self::Flake("nixpkgs".to_owned()),
        }
    }

    /// The arguments that make `nix shell` or `nix build` use `attrs` from
    /// this nixpkgs
    fn installable_args(&self, attrs: &[&str]) -> Vec<String> {
        match self {
            Self::Channel { .. } => ["-f", "<nixpkgs>"]
                .iter()
                .chain(attrs)
                .map(|&arg| arg.to_owned())
                .collect(),
            Self::Flake(flake) => attrs.iter().map(|attr| format!("{flake}#{attr}")).collect(),
        }
    }

    /// The key that paths built from this nixpkgs are cached under.
    /// `<nixpkgs>` is resolved through `NIX_PATH`, so the key contains the
    /// entry that provides it. Other entries don't matter: changing
    /// `nixos-config=` shouldn't make comma evaluate every command again
    fn cache_key(&self) -> String {
        match self {
            Self::Channel { nix_path } => match nixpkgs_entry(nix_path) {
                Some(value) => format!("<nixpkgs> ({value})"),
                None => format!("<nixpkgs> (NIX_PATH={nix_path})"),
            },
            Self::Flake(flake) => flake.clone(),
        }
    }
}

/// Whether nix treats `s` as a URL in `NIX_PATH`, like `isPseudoUrl` in nix
fn is_pseudo_url(s: &str) -> bool {
    s.starts_with("channel:")
        || s.split_once("://").is_some_and(|(scheme, _)| {
            matches!(
                scheme,
                "http" | "https" | "file" | "channel" | "git" | "s3" | "ssh"
            )
        })
}

/// Split `NIX_PATH` into its entries the way nix does (`parseNixPath`).
/// Entries are separated by `:`, but the first `:` of a value like
/// `https://...`, `channel:nixos-unstable` or `flake:nixpkgs` belongs to the
/// value. Empty entries are left out, they don't show up in
/// `builtins.nixPath` either
fn split_nix_path(nix_path: &str) -> Vec<&str> {
    let mut entries = Vec::new();
    let mut rest = nix_path;

    while !rest.is_empty() {
        let Some(colon) = rest.find(':') else {
            entries.push(rest);
            break;
        };

        // Like nix, look at everything after the `=` of this entry, which
        // may reach into the entries that follow
        let value = &rest[rest[..colon].rfind('=').map_or(0, |i| i + 1)..];
        let end = if is_pseudo_url(value) || value.starts_with("flake:") {
            rest[colon + 1..]
                .find(':')
                .map_or(rest.len(), |i| colon + 1 + i)
        } else {
            colon
        };

        if end > 0 {
            entries.push(&rest[..end]);
        }
        rest = rest.get(end + 1..).unwrap_or_default();
    }

    entries
}

/// The value of the `nixpkgs=` entry of `nix_path` that `<nixpkgs>` resolves
/// to. `None` if it can't be told from `NIX_PATH` alone: nix uses the first
/// entry that has the path, and an entry without a name, a plain directory,
/// has it if it contains a `nixpkgs` directory
fn nixpkgs_entry(nix_path: &str) -> Option<&str> {
    split_nix_path(nix_path)
        .into_iter()
        .find_map(|entry| match entry.split_once('=') {
            Some(("nixpkgs", value)) => Some(Some(value)),
            Some(_) => None,
            None => Some(None),
        })
        .flatten()
}

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
            .map(std::borrow::ToOwned::to_owned)
            .collect(),
    )
}

/// What is known about the package of a command before the user is asked
enum Found {
    /// The choice that is remembered in the cache
    Remembered(CacheEntry),
    /// The packages in the index that provide the command
    InIndex(Box<[String]>),
}

/// Look `command` up in `cache` and, when it isn't remembered there, in the
/// index
fn find_command(cache: Option<&Cache>, command: &str) -> Option<Found> {
    match cache.and_then(|cache| cache.query(command)) {
        Some(entry) => Some(Found::Remembered(entry)),
        None => index_database(command).map(Found::InIndex),
    }
}

/// The package for `command` out of what was `found` for it, and whether the
/// user chose it in the picker just now, rather than it being the only one or
/// remembered in the cache. The picker is only shown when there are several
/// packages. A package from the index is remembered in `cache`
fn choose_entry(
    cache: Option<&mut Cache>,
    command: &str,
    found: &Found,
    picker: &str,
) -> Option<(CacheEntry, bool)> {
    let attrs = match found {
        Found::Remembered(entry) => return Some((entry.clone(), false)),
        Found::InIndex(attrs) => attrs,
    };

    let (derivation, chosen) = if attrs.len() > 1 {
        (pick(picker, attrs)?, true)
    } else {
        (attrs.first()?.trim().to_owned(), false)
    };
    let entry = CacheEntry {
        derivation,
        paths: HashMap::new(),
    };
    if let Some(cache) = cache {
        cache.update(command, entry.clone());
    }
    Some((entry, chosen))
}

/// The package that provides `command`, and whether the user chose it in the
/// picker just now
fn find_entry(
    cache: Option<&mut Cache>,
    command: &str,
    picker: &str,
) -> Option<(CacheEntry, bool)> {
    let found = find_command(cache.as_deref(), command)?;
    choose_entry(cache, command, &found, picker)
}

/// `nix shell` with the packages `choices`, running `command` with the
/// arguments `trail` in it
fn run_command_or_open_shell(
    nixpkgs: &Nixpkgs,
    choices: &[&str],
    command: &str,
    trail: &[&str],
) -> Command {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "shell",
    ]);

    run_cmd.args(nixpkgs.installable_args(choices));

    if !command.is_empty() {
        run_cmd.args(["--command", command]);
        if !trail.is_empty() {
            run_cmd.args(trail);
        }
    }

    trace!("run nix command arguments: {run_cmd:?}");

    run_cmd
}

/// The exit statuses a shell gives for a command that wasn't found, and for
/// one that was found but couldn't be executed
const EXIT_NOT_FOUND: u8 = 127;
const EXIT_NOT_EXECUTABLE: u8 = 126;

/// The exit status for `program`, which couldn't be executed because of
/// `err`. These are the ones a shell gives, so that this can be told apart
/// from the program itself failing
fn exec_failure_status(program: &Path, err: &io::Error) -> u8 {
    // A program that exists is "not found" when its interpreter is missing
    // (the one after `#!`, or the ELF loader). A shell says for that, that
    // the program couldn't be executed. A name without a directory is
    // searched in PATH, so it really wasn't found
    let exists = program.is_absolute() && program.exists();

    if err.kind() == io::ErrorKind::NotFound && !exists {
        EXIT_NOT_FOUND
    } else {
        EXIT_NOT_EXECUTABLE
    }
}

/// Replace this process with `cmd`. Only returns if that failed. Destructors
/// don't run when it succeeds, so `cache` is taken and dropped first, which
/// writes the cache file
fn exec(mut cmd: Command, cache: Option<Cache>) -> ExitCode {
    drop(cache);

    let err = cmd.exec();
    eprintln!(
        "comma: failed to execute '{}': {err}",
        cmd.get_program().to_string_lossy()
    );
    ExitCode::from(exec_failure_status(Path::new(cmd.get_program()), &err))
}

/// The commands whose packages go in the shell, each of them once: with
/// --shell every argument is a command
fn shell_commands(args: &Opt) -> Vec<&str> {
    let mut given: Vec<&str> = args.cmd.iter().map(String::as_str).collect();

    // `man` in front of the other commands was parsed as the subcommand
    if let Some(SubCmds::Man(ManArgs { cmd })) = &args.subcmds {
        given.push("man");
        given.extend(cmd.iter().map(String::as_str));
    }

    let mut commands = Vec::new();
    for command in given {
        if !commands.contains(&command) {
            commands.push(command);
        }
    }
    commands
}

/// Open a shell that has the packages of all the commands given in `args`
fn open_shell(args: &Opt, mut cache: Option<Cache>, nixpkgs: &Nixpkgs) -> ExitCode {
    let commands = shell_commands(args);

    // All the commands are looked up before the package of any of them is
    // asked for, so that the commands that don't exist are reported right
    // away, all of them, and not after the pickers of the commands before
    // them. With --delete-entry the remembered choices aren't used
    let remembered = cache.as_ref().filter(|_| !args.delete_entry);
    let found: Vec<Option<Found>> = commands
        .iter()
        .map(|command| find_command(remembered, command))
        .collect();
    let Some(found) = found.into_iter().collect::<Option<Vec<Found>>>() else {
        return ExitCode::FAILURE;
    };

    // Only now, so that the choices aren't forgotten for nothing when a
    // command doesn't exist
    if args.delete_entry {
        if let Some(cache) = cache.as_mut() {
            for command in &commands {
                cache.delete(command);
            }
        }
    }

    // The same package may provide several of the commands
    let mut derivations: Vec<String> = Vec::new();
    for (command, found) in commands.iter().zip(&found) {
        // The picker only shows the packages. With a single command it is
        // clear what they are for
        if commands.len() > 1 && matches!(found, Found::InIndex(attrs) if attrs.len() > 1) {
            eprintln!("Choose the package for '{command}':");
        }
        let Some((entry, _)) = choose_entry(cache.as_mut(), command, found, &args.picker) else {
            return ExitCode::FAILURE;
        };
        if !derivations.contains(&entry.derivation) {
            derivations.push(entry.derivation);
        }
    }
    let derivations: Vec<&str> = derivations.iter().map(String::as_str).collect();

    // TODO: use cache here, but this is tricky since it actually depends in `nix-shell`
    let shell_cmd = shell::select_shell_from_pid(process::id()).unwrap_or("bash".into());
    let run_cmd = run_command_or_open_shell(nixpkgs, &derivations, &shell_cmd, &[]);

    exec(run_cmd, cache)
}

fn get_command_path(nixpkgs: &Nixpkgs, choice: &str, command: &str) -> Option<String> {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "build",
        "--print-out-paths",
        "--no-link",
    ]);

    run_cmd.args(nixpkgs.installable_args(&[choice]));

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
    nixpkgs: &Nixpkgs,
    command: &str,
) -> Option<String> {
    let source = nixpkgs.cache_key();
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
            let path = get_command_path(nixpkgs, &entry.derivation, command)?;
            debug!("found path from nix for command '{command}' using '{source}': {path}");

            if let Some(cache) = cache {
                let mut entry = entry.clone();
                entry.set_path(source, path.clone());
                cache.update(command, entry);
            }

            Some(path)
        }
    }
}

fn run_command_from_cache(
    cache: Option<&mut Cache>,
    entry: &CacheEntry,
    nixpkgs: &Nixpkgs,
    command: &str,
    trail: &[String],
) -> Option<Command> {
    let path = get_command_path_from_cache(cache, entry, nixpkgs, command)?;

    let mut run_cmd = Command::new(path);
    if !trail.is_empty() {
        run_cmd.args(trail);
    }

    trace!("run command from cache arguments: {run_cmd:?}");

    Some(run_cmd)
}

/// The exit status when the user declined to run the command. It is what a
/// shell gives for a command that wasn't found, so that after a declined
/// command-not-found `typo && other` doesn't go on to run `other`
const EXIT_NOT_RUN: u8 = EXIT_NOT_FOUND;

/// Ask whether to run `command` from the package `derivation`, with the
/// question written to `output` and the answer read from `input`. This is
/// asked before the package is built, so its store path isn't known yet
fn confirm(
    input: &mut impl BufRead,
    output: &mut impl Write,
    command: &str,
    derivation: &str,
) -> io::Result<bool> {
    loop {
        write!(output, "Run '{command}' from '{derivation}'? [Y/n]: ")?;
        output.flush()?;

        let mut answer = String::new();
        if input.read_line(&mut answer)? == 0 {
            // The input ended without an answer (Ctrl-D). Only an empty line
            // means yes. End the line of the question, as Enter would have
            writeln!(output)?;
            return Ok(false);
        }

        match answer.trim().to_lowercase().as_str() {
            "y" | "yes" | "" => return Ok(true),
            "n" | "no" => return Ok(false),
            _ => writeln!(output, "Please enter 'y' or 'n'.")?,
        }
    }
}

/// Ask on the terminal whether to run `command` from the package
/// `derivation`. Without a terminal to ask on, the answer is no
fn confirmer(command: &str, derivation: &str) -> bool {
    // stdin and stdout belong to the command and may be pipes
    // (`cat file | cmd`, `cmd | less`), and a command-not-found handler may
    // capture stdout. So ask on the terminal itself, like ssh and sudo do
    OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
        .and_then(|tty| confirm(&mut BufReader::new(&tty), &mut &tty, command, derivation))
        .unwrap_or_else(|err| {
            eprintln!(
                "comma: not running '{command}' from '{derivation}': \
                 can't ask for confirmation on the terminal: {err}"
            );
            false
        })
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
        return if let Some(ref cache) = cache {
            print_cache(cache);
            ExitCode::SUCCESS
        } else {
            eprintln!("The cache is disabled");
            ExitCode::FAILURE
        };
    }

    if args.cmd.is_empty() && args.subcmds.is_none() {
        return if args.empty_cache {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let nixpkgs = Nixpkgs::from_args(args.nixpkgs_flake.as_deref());

    // --shell is a mode of its own: it takes several commands, where
    // everything below is about a single one
    if args.shell {
        return open_shell(&args, cache, &nixpkgs);
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

    let Some((entry, picked)) = find_entry(cache.as_mut(), command, &args.picker) else {
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

    if args.install {
        let mut run_cmd = Command::new("nix-env");
        run_cmd.args(["-f", "<nixpkgs>", "-iA", basename]);

        exec(run_cmd, cache)
    } else if args.print_path {
        let Some(path) = get_command_path_from_cache(path_cache, &entry, &nixpkgs, command) else {
            return ExitCode::FAILURE;
        };
        println!("{path}");

        ExitCode::SUCCESS
    } else if args.subcmds.is_some() {
        // Open manpage via
        // nix shell nixpkgs#drvName --command man commandName
        let run_cmd = run_command_or_open_shell(
            &nixpkgs,
            &[&entry.derivation.replace(".out", "^*")],
            "man",
            &[command],
        );

        exec(run_cmd, cache)
    } else {
        // Ask before the package is built or downloaded. Choosing it in the
        // picker was already the confirmation
        if args.ask && !picked && !confirmer(command, &entry.derivation) {
            return ExitCode::from(EXIT_NOT_RUN);
        }

        let Some(run_cmd) = run_command_from_cache(path_cache, &entry, &nixpkgs, command, trail)
        else {
            return ExitCode::FAILURE;
        };

        exec(run_cmd, cache)
    }
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

    /// Open a shell containing the derivations containing the executables.
    /// Every argument is an executable, e.g. `, -s cowsay lolcat`
    #[clap(short, long, conflicts_with_all = ["install", "print_packages", "print_path"])]
    shell: bool,

    /// Picker to use
    #[clap(short = 'P', long, env = "COMMA_PICKER", default_value = "fzy")]
    picker: String,

    /// Nixpkgs flake to use, e.g. `github:nixos/nixpkgs/nixos-unstable` [default: nixpkgs].
    /// When given, it takes precedence over `<nixpkgs>` from `NIX_PATH`
    #[clap(short = 'F', long, env = "COMMA_NIXPKGS_FLAKE")]
    nixpkgs_flake: Option<String>,

    /// Ask to confirm the program that will be run, before its package is
    /// downloaded. Not asked right after choosing the package in the picker.
    /// The question is asked on the terminal; without one the program isn't
    /// run. When it isn't run, comma exits with status 127
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
    /// `COMMA_NO_CACHE_PATH` environment variable (comma separated) are treated
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

    /// Command to run, followed by its arguments. With `--shell`, the
    /// commands whose packages go in the shell
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

#[cfg(test)]
mod tests {
    use super::*;

    fn channel(nix_path: &str) -> Nixpkgs {
        Nixpkgs::Channel {
            nix_path: nix_path.to_owned(),
        }
    }

    fn flake(flake: &str) -> Nixpkgs {
        Nixpkgs::Flake(flake.to_owned())
    }

    /// The result of `confirm` for the given input, and what it wrote
    fn confirm_with(input: &str) -> (bool, String) {
        let mut output = Vec::new();
        let answer = confirm(&mut input.as_bytes(), &mut output, "sl", "sl.out").unwrap();
        (answer, String::from_utf8(output).unwrap())
    }

    #[test]
    fn confirm_answers() {
        let question = "Run 'sl' from 'sl.out'? [Y/n]: ";

        // An empty line is the default, yes
        assert_eq!(confirm_with("y\n"), (true, question.to_owned()));
        assert_eq!(confirm_with("\n"), (true, question.to_owned()));
        assert_eq!(confirm_with("n\n"), (false, question.to_owned()));

        // Without an answer the program isn't run
        assert_eq!(confirm_with(""), (false, format!("{question}\n")));

        // Asked again after anything else
        assert_eq!(
            confirm_with("maybe\ny\n"),
            (true, format!("{question}Please enter 'y' or 'n'.\n{question}"))
        );
    }

    #[test]
    fn exec_failure_statuses() {
        let not_found = io::Error::from(io::ErrorKind::NotFound);

        // A name is searched in PATH, whatever is in the current directory
        assert_eq!(exec_failure_status(Path::new("src"), &not_found), 127);
        assert_eq!(
            exec_failure_status(Path::new("/no/such/program"), &not_found),
            127
        );
        // It exists, so what wasn't found is its interpreter
        assert_eq!(exec_failure_status(Path::new("/"), &not_found), 126);
    }

    #[test]
    fn nixpkgs_installable_args() {
        assert_eq!(
            channel("nixpkgs=/nix/nixpkgs").installable_args(&["cowsay.out", "lolcat.out"]),
            ["-f", "<nixpkgs>", "cowsay.out", "lolcat.out"]
        );
        assert_eq!(
            flake("nixpkgs").installable_args(&["cowsay.out", "lolcat.out"]),
            ["nixpkgs#cowsay.out", "nixpkgs#lolcat.out"]
        );
    }

    #[test]
    fn shell_commands_are_all_arguments() {
        let commands = |args: &[&str]| {
            let args = Opt::try_parse_from([&[","], args].concat()).unwrap();
            shell_commands(&args)
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        };

        assert_eq!(commands(&["-s", "cowsay", "lolcat"]), ["cowsay", "lolcat"]);
        // --shell is a flag, so it can be combined with other short flags
        assert_eq!(commands(&["-sd", "cowsay"]), ["cowsay"]);
        // A command that is given twice is there once
        assert_eq!(
            commands(&["-s", "cowsay", "lolcat", "cowsay"]),
            ["cowsay", "lolcat"]
        );
        // `man` is a command like the others, also in front of them, where
        // it is parsed as the subcommand
        assert_eq!(commands(&["-s", "cowsay", "man"]), ["cowsay", "man"]);
        assert_eq!(
            commands(&["-s", "man", "cowsay", "lolcat"]),
            ["man", "cowsay", "lolcat"]
        );
    }

    #[test]
    fn nixpkgs_resolve() {
        let nix_path = "nixpkgs=/nix/nixpkgs:nixos-config=/etc/nixos";

        assert_eq!(
            Nixpkgs::resolve(None, nix_path.to_owned()),
            channel(nix_path)
        );
        assert_eq!(Nixpkgs::resolve(None, String::new()), flake("nixpkgs"));

        // An explicit flake wins over NIX_PATH
        assert_eq!(
            Nixpkgs::resolve(
                Some("github:nixos/nixpkgs/nixos-unstable"),
                nix_path.to_owned()
            ),
            flake("github:nixos/nixpkgs/nixos-unstable")
        );

        // An empty flake counts as not given
        assert_eq!(
            Nixpkgs::resolve(Some(""), nix_path.to_owned()),
            channel(nix_path)
        );
    }

    #[test]
    fn nix_path_entries() {
        // (NIX_PATH, its entries, the value `<nixpkgs>` resolves to)
        let cases: &[(&str, &[&str], Option<&str>)] = &[
            (
                "nixos-config=/etc/nixos/configuration.nix:nixpkgs=/nix/nixpkgs:/nix/channels",
                &[
                    "nixos-config=/etc/nixos/configuration.nix",
                    "nixpkgs=/nix/nixpkgs",
                    "/nix/channels",
                ],
                Some("/nix/nixpkgs"),
            ),
            // The first `:` of these values doesn't end the entry
            (
                "nixpkgs=https://example.org/nixpkgs.tar.gz:nixos-config=/etc/nixos",
                &[
                    "nixpkgs=https://example.org/nixpkgs.tar.gz",
                    "nixos-config=/etc/nixos",
                ],
                Some("https://example.org/nixpkgs.tar.gz"),
            ),
            (
                "nixpkgs=channel:nixos-unstable:nixos-config=/etc/nixos",
                &["nixpkgs=channel:nixos-unstable", "nixos-config=/etc/nixos"],
                Some("channel:nixos-unstable"),
            ),
            (
                "nixos-config=/etc/nixos:nixpkgs=flake:nixpkgs",
                &["nixos-config=/etc/nixos", "nixpkgs=flake:nixpkgs"],
                Some("flake:nixpkgs"),
            ),
            // Empty entries are left out
            (
                ":nixos-config=/etc/nixos::nixpkgs=/nix/nixpkgs:",
                &["nixos-config=/etc/nixos", "nixpkgs=/nix/nixpkgs"],
                Some("/nix/nixpkgs"),
            ),
            // A directory before the entry may contain `nixpkgs`
            (
                "/nix/channels:nixpkgs=/nix/nixpkgs",
                &["/nix/channels", "nixpkgs=/nix/nixpkgs"],
                None,
            ),
            // The first of several entries is used
            (
                "nixpkgs=/first:nixpkgs=/second",
                &["nixpkgs=/first", "nixpkgs=/second"],
                Some("/first"),
            ),
            // No entry for nixpkgs
            ("mynixpkgs=/x", &["mynixpkgs=/x"], None),
        ];

        for (nix_path, entries, nixpkgs) in cases {
            assert_eq!(split_nix_path(nix_path), *entries, "{nix_path}");
            assert_eq!(nixpkgs_entry(nix_path), *nixpkgs, "{nix_path}");
        }
    }

    #[test]
    fn nixpkgs_cache_key() {
        // Only the entry for nixpkgs is part of the key
        assert_eq!(
            channel("nixpkgs=/nix/nixpkgs:nixos-config=/etc/nixos").cache_key(),
            channel("nixos-config=/other:nixpkgs=/nix/nixpkgs").cache_key(),
        );
        assert_eq!(
            channel("nixpkgs=/nix/nixpkgs:nixos-config=/etc/nixos").cache_key(),
            "<nixpkgs> (/nix/nixpkgs)"
        );
        // The whole NIX_PATH, when that entry can't be told
        assert_eq!(
            channel("/nix/channels:nixpkgs=/nix/nixpkgs").cache_key(),
            "<nixpkgs> (NIX_PATH=/nix/channels:nixpkgs=/nix/nixpkgs)"
        );
        assert_eq!(
            flake("github:nixos/nixpkgs/nixos-unstable").cache_key(),
            "github:nixos/nixpkgs/nixos-unstable"
        );
    }
}
