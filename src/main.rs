mod cache;
mod cli;
mod index;
mod install;
mod nixpkgs;
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
use clap::{CommandFactory, Parser};
use clap_complete::{generate, Generator};
use cli::{ManArgs, Opt, SubCmds};
use install::Installer;
use log::{debug, error, trace};
use nixpkgs::Nixpkgs;

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

/// The packages in the nix-index database that have `command`, or the
/// reason why there are none
fn locate(command: &str) -> Result<Box<[String]>, String> {
    let nix_locate_output = Command::new("nix-locate")
        .args(["--minimal", "--at-root", "--whole-name"])
        .arg(format!("/bin/{command}"))
        .output()
        .map_err(|err| format!("failed to execute nix-locate: {err}"))?;

    if !nix_locate_output.status.success() {
        return Err(match std::str::from_utf8(&nix_locate_output.stderr) {
            Ok(stderr) => format!("nix-locate failed with: {stderr}"),
            Err(_) => "nix-locate failed".to_owned(),
        });
    }

    let attrs = nix_locate_output.stdout;

    if attrs.is_empty() {
        return Err(format!(
            "No executable `{command}` found in nix-index database."
        ));
    }

    Ok(std::str::from_utf8(&attrs)
        .expect("fail")
        .trim()
        .split('\n')
        .map(std::borrow::ToOwned::to_owned)
        .collect())
}

fn index_database(command: &str) -> Option<Box<[String]>> {
    index::check_database_updated();

    locate(command).inspect_err(|err| eprintln!("{err}")).ok()
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

/// The installable for all outputs of the package of `derivation`, which
/// nix-locate gives with the output that has the command appended, like
/// `micro.out` or `bind.dnsutils`. The man page can be in another output
fn all_outputs(derivation: &str) -> String {
    let attr = derivation
        .rsplit_once('.')
        .map_or(derivation, |(attr, _output)| attr);
    format!("{attr}^*")
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

/// The package that a command is run from in a shell with `packages`, when
/// that isn't the package `chosen` for it. The packages come first in PATH in
/// their order, so another one of the `providers`, the packages that have the
/// command, wins when it is before the chosen one.
///
/// That is the order since nix 2.20:
/// <https://github.com/nixos/nix/commit/f4454aac9f002c33b84bb52e0bba2a46ea6170cd>.
/// Before it, `nix shell` put every package in front of the ones before it,
/// so the last one won. With such a nix this is wrong about which package a
/// command is run from
fn shadowing_package<'a>(
    packages: &[&'a str],
    chosen: &str,
    providers: &[String],
) -> Option<&'a str> {
    packages
        .iter()
        .copied()
        .take_while(|package| *package != chosen)
        .find(|package| providers.iter().any(|provider| provider == package))
}

/// Warn about the `commands` that aren't run from the package `chosen` for
/// them in a shell with `packages`
fn warn_about_shadowed(commands: &[&str], found: &[Found], chosen: &[String], packages: &[&str]) {
    for ((command, found), chosen) in commands.iter().zip(found).zip(chosen) {
        // Nothing is before the first package
        if packages.first() == Some(&chosen.as_str()) {
            continue;
        }

        // The index wasn't looked at for a remembered choice. When that
        // doesn't work now, there is nothing to warn about
        let providers = match found {
            Found::InIndex(attrs) => attrs.clone(),
            Found::Remembered(_) => locate(command).unwrap_or_default(),
        };

        if let Some(package) = shadowing_package(packages, chosen, &providers) {
            eprintln!(
                "Warning: '{command}' in the shell is the one from '{package}', \
                 which comes before '{chosen}' in PATH."
            );
        }
    }
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

    let mut chosen: Vec<String> = Vec::new();
    for (command, found) in commands.iter().zip(&found) {
        // The picker only shows the packages. With a single command it is
        // clear what they are for
        if commands.len() > 1 && matches!(found, Found::InIndex(attrs) if attrs.len() > 1) {
            eprintln!("Choose the package for '{command}':");
        }
        let Some((entry, _)) = choose_entry(cache.as_mut(), command, found, &args.picker) else {
            return ExitCode::FAILURE;
        };
        chosen.push(entry.derivation);
    }

    // The same package may provide several of the commands
    let mut derivations: Vec<&str> = Vec::new();
    for package in &chosen {
        if !derivations.contains(&package.as_str()) {
            derivations.push(package);
        }
    }

    warn_about_shadowed(&commands, &found, &chosen, &derivations);

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
            .and_then(|p| {
                Path::new(&p)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
            })
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

    // The cache used for path lookups; the choice is still kept in `cache`
    let path_cache = cache.as_mut().filter(|_| cache_paths);

    if args.install {
        let install_cmd = Installer::detect().command(&nixpkgs, &entry.derivation);

        exec(install_cmd, cache)
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
            &[&all_outputs(&entry.derivation)],
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
    generate(generator, cmd, bin_name, &mut io::stdout());
}

#[cfg(test)]
mod tests {
    use super::*;

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
            (
                true,
                format!("{question}Please enter 'y' or 'n'.\n{question}")
            )
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
    fn all_outputs_of_package() {
        assert_eq!(all_outputs("micro.out"), "micro^*");
        // The command can be in an output other than `out`
        assert_eq!(all_outputs("bind.dnsutils"), "bind^*");
        // Only the output is replaced, not `.out` inside the attribute path
        assert_eq!(
            all_outputs("python3Packages.outcome.out"),
            "python3Packages.outcome^*"
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
    fn shadowing_packages() {
        let packages = ["xz.bin", "busybox.out", "lolcat.out"];
        let xz = ["xz.bin".to_owned(), "busybox.out".to_owned()];
        let lolcat = ["lolcat.out".to_owned()];

        // xz.bin has xz too and is before busybox.out
        assert_eq!(
            shadowing_package(&packages, "busybox.out", &xz),
            Some("xz.bin")
        );
        // The chosen package is before the other one that has it
        assert_eq!(shadowing_package(&packages, "xz.bin", &xz), None);
        // No other package has it
        assert_eq!(shadowing_package(&packages, "lolcat.out", &lolcat), None);
    }
}
