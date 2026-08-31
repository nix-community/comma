mod cache;
mod index;
mod shell;

use std::{
    collections::HashSet,
    env,
    ffi::OsStr,
    io::{self, Write},
    os::unix::prelude::CommandExt,
    path::Path,
    process::{self, Command, ExitCode, Stdio},
};

use cache::{Cache, CacheEntry};
use clap::{crate_version, Args, CommandFactory, Parser, Subcommand, ValueHint};
use clap_complete::{
    engine::{ArgValueCompleter, CompletionCandidate},
    generate, CompleteEnv, Generator, Shell,
};
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

/// Shell completion for the command to run, backed by the nix-index database.
///
/// Looks up executables in `/bin` whose name starts with `current` (e.g. typing
/// `, ra` and pressing tab could suggest `rar`, `rare`, `rars`, ...), using the
/// same prefix matching semantics as `nix-locate --at-root` (anchors the
/// pattern at the start of the path but not the end).
fn complete_command(current: &OsStr) -> Vec<CompletionCandidate> {
    let Some(current) = current.to_str() else {
        return Vec::new();
    };

    let Ok(nix_locate_output) = Command::new("nix-locate")
        .args(["--at-root"])
        .arg(format!("/bin/{current}"))
        .output()
    else {
        return Vec::new();
    };

    if !nix_locate_output.status.success() {
        return Vec::new();
    }

    let Ok(stdout) = std::str::from_utf8(&nix_locate_output.stdout) else {
        return Vec::new();
    };

    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for line in stdout.lines() {
        // Each line looks like:
        // <attr>   <size>   <type>   <store path>
        // where <store path> ends with `/bin/<name>`.
        let mut fields = line.split_whitespace();
        let Some(attr) = fields.next() else { continue };
        let (Some(_size), Some(_kind), Some(path)) = (fields.next(), fields.next(), fields.next())
        else {
            continue;
        };

        let Some(name) = path.rsplit('/').next() else {
            continue;
        };
        if name.is_empty() || !name.starts_with(current) {
            continue;
        }

        if !seen.insert(name.to_owned()) {
            continue;
        }

        let attr = attr.trim_start_matches('(').trim_end_matches(')');
        let package = attr.rsplit_once('.').map_or(attr, |(base, _)| base);

        candidates.push(CompletionCandidate::new(name).help(Some(package.to_owned().into())));
    }

    candidates
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

fn get_command_path(use_channel: bool, choice: &str, command: &str, nixpkgs_flake: &str) -> String {
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
    let output = result.wait_with_output().unwrap().stdout;
    let base_path = std::str::from_utf8(&output)
        .unwrap_or_else(|err| panic!("nix outputted invalid UTF-8: {err}"))
        .trim();

    // It is safe to assume that command is in $out/bin/{command} from
    // the derivation, since this was already filtered by nix-locate
    format!("{base_path}/bin/{command}")
}

fn get_command_path_from_cache(
    cache: &mut Option<Cache>,
    entry: &CacheEntry,
    use_channel: bool,
    command: &str,
    nixpkgs_flake: &str,
) -> String {
    match &entry.path {
        // If we have the path in the cache and it is not garbage collected
        // (so the path still exists), it should be safe to use it directly
        Some(path) if Path::new(&path).exists() => {
            debug!("found path from cache for command '{command}': {path}");
            path.to_owned()
        }
        // Otherwise, we need to find the command path
        _ => match cache {
            Some(ref mut cache) => {
                let path = get_command_path(use_channel, &entry.derivation, command, nixpkgs_flake);
                debug!("found path from nix for command '{command}': {path}");

                let entry = CacheEntry {
                    path: Some(path.clone()),
                    ..entry.clone()
                };
                cache.update(command, entry);

                path
            }

            None => {
                let path = get_command_path(use_channel, &entry.derivation, command, nixpkgs_flake);
                debug!("found path from nix for command '{command}': {path}");

                path
            }
        },
    }
}

fn run_command_from_cache(
    cache: &mut Option<Cache>,
    entry: &CacheEntry,
    use_channel: bool,
    command: &str,
    trail: &[String],
    nixpkgs_flake: &str,
) -> Command {
    let path = get_command_path_from_cache(cache, entry, use_channel, command, nixpkgs_flake);

    let mut run_cmd = Command::new(path);
    if !trail.is_empty() {
        run_cmd.args(trail);
    }

    trace!("run command from cache arguments: {run_cmd:?}");

    run_cmd
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
    let bin_name = std::env::args()
        .next()
        .and_then(|p| {
            Path::new(&p)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "comma".into());

    // Must run before any output is written to stdout, and before argument
    // parsing, since it handles `COMPLETE=<shell> <bin>`-activated shell
    // completion requests and exits early when one is being served.
    CompleteEnv::with_factory(Opt::command)
        .bin(bin_name.clone())
        .complete();

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

    if args.command.is_none() && args.subcmds.is_none() {
        return if args.empty_cache {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    let (command, trail): (&String, &[String]) =
        if let Some(SubCmds::Man(ManArgs { ref cmd })) = args.subcmds {
            (&cmd[0], &cmd[1..])
        } else {
            (
                args.command.as_ref().expect("command is required"),
                args.trail.as_slice(),
            )
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
                    path: None,
                };
                cache.update(command, entry.clone());
                entry
            })
        }),
        None => index_database_pick(command, &args.picker).map(|derivation| CacheEntry {
            derivation,
            path: None,
        }),
    };

    let entry = match entry {
        Some(d) if args.cache_level >= 2 => d,
        Some(d) => {
            debug!("cache_level={}, ignoring path from cache", args.cache_level);
            CacheEntry {
                derivation: d.derivation.clone(),
                path: None,
            }
        }
        None => return ExitCode::FAILURE,
    };

    let basename = entry.derivation.rsplit('.').next_back().unwrap();

    let use_channel = env::var("NIX_PATH")
        .unwrap_or_default()
        .contains("nixpkgs=");

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
            &args.nixpkgs_flake,
        )
        .exec();
    } else if args.print_path {
        let path = get_command_path_from_cache(
            &mut cache,
            &entry,
            use_channel,
            command,
            &args.nixpkgs_flake,
        );
        println!("{path}");
    } else if args.subcmds.is_some() {
        // Open manpage via
        // nix shell nixpkgs#drvName --command man commandName
        let err = run_command_or_open_shell(
            use_channel,
            &entry.derivation.replace(".out", "^*"),
            "man",
            &[command],
            &args.nixpkgs_flake,
        )
        .exec();

        // This code will only run if an error occurs launching
        eprintln!("{err:?}");
        return ExitCode::FAILURE;
    } else {
        let mut run_cmd = run_command_from_cache(
            &mut cache,
            &entry,
            use_channel,
            command,
            trail,
            &args.nixpkgs_flake,
        );

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
#[clap(version = crate_version!(), trailing_var_arg = true)]
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

    /// Nixpkgs flake to use
    #[clap(
        short = 'F',
        long,
        env = "COMMA_NIXPKGS_FLAKE",
        default_value = "nixpkgs"
    )]
    nixpkgs_flake: String,

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
    /// choices, 2 also caches path evaluations
    #[clap(long = "cache-level", env = "COMMA_CACHING", default_value_t = 2)]
    cache_level: u8,

    /// Empty the cache
    #[clap(short, long = "empty-cache")]
    empty_cache: bool,

    /// Overwrite the cache entry for the specified command. This is achieved
    /// by first deleting it from the cache, then running comma as normal
    #[clap(short, long = "delete-entry")]
    delete_entry: bool,

    /// Command to run
    #[clap(
        required_unless_present_any = ["empty_cache", "mangen", "print_completions"],
        name = "cmd",
        value_hint = ValueHint::Other,
        add = ArgValueCompleter::new(complete_command),
    )]
    command: Option<String>,

    /// Arguments passed to the command
    #[clap(name = "trail", value_hint = ValueHint::Other, allow_hyphen_values = true)]
    trail: Vec<String>,

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
    #[clap(required = true, name = "cmd", add = ArgValueCompleter::new(complete_command))]
    cmd: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::Opt;
    use clap::Parser;

    #[test]
    fn parses_command_and_trail() {
        let opt = Opt::parse_from(["comma", "ls", "-la", "foo"]);
        assert_eq!(opt.command.as_deref(), Some("ls"));
        assert_eq!(opt.trail, vec!["-la".to_string(), "foo".to_string()]);
    }

    #[test]
    fn parses_flags_before_command() {
        let opt = Opt::parse_from(["comma", "--install", "ls"]);
        assert!(opt.install);
        assert_eq!(opt.command.as_deref(), Some("ls"));
        assert!(opt.trail.is_empty());
    }

    #[test]
    fn empty_cache_alone_is_valid() {
        let opt = Opt::parse_from(["comma", "--empty-cache"]);
        assert!(opt.command.is_none());
        assert!(opt.empty_cache);
    }

    #[test]
    fn missing_command_is_error() {
        assert!(Opt::try_parse_from(["comma"]).is_err());
    }
}

#[cfg(test)]
mod complete_command_tests {
    use super::complete_command;
    use std::{ffi::OsStr, io::Write, os::unix::fs::PermissionsExt};

    /// Puts a fake `nix-locate` executable on `PATH` for the duration of the
    /// test, producing the given canned output.
    fn with_fake_nix_locate<T>(output: &str, test: impl FnOnce() -> T) -> T {
        let dir = std::env::temp_dir().join(format!("comma-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let script_path = dir.join("nix-locate");
        {
            let mut script = std::fs::File::create(&script_path).unwrap();
            writeln!(script, "#!/bin/sh").unwrap();
            writeln!(script, "cat <<'DATA'\n{output}DATA").unwrap();
        }
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o755)).unwrap();

        let old_path = std::env::var_os("PATH");
        let new_path = match &old_path {
            Some(p) => format!("{}:{}", dir.display(), p.to_string_lossy()),
            None => dir.display().to_string(),
        };
        // SAFETY: tests in this module do not run concurrently with code that
        // reads `PATH` (all such reads happen within `test`, spawned below).
        unsafe { std::env::set_var("PATH", new_path) };

        let result = test();

        match old_path {
            Some(p) => unsafe { std::env::set_var("PATH", p) },
            None => unsafe { std::env::remove_var("PATH") },
        }
        let _ = std::fs::remove_dir_all(&dir);

        result
    }

    const SAMPLE_OUTPUT: &str = "\
rare-regex.out                            5,659,424 x /nix/store/kkdvcdd51yw0jb59h8mihqj0mdbm2k5f-rare-regex-0.5.2/bin/rare
rare.out                                     20,560 x /nix/store/habbma2yyhm8yp120f0srziy8lm0vq4y-rare-1.10.11/bin/rare
rars.out                                        239 x /nix/store/zwiml4j4ddmh06321b1gph04dy5asyfp-rars-1.6/bin/rars
rar2hashcat.out                              69,184 x /nix/store/jswfx1637ap5zrpvynf5h295p6dhzm0h-rar2hashcat-1.0/bin/rar2hashcat
john.out                                          0 s /nix/store/dj3cs5ncnzg2jgfprjlifg9knyfjkq3z-john-1.9.0-Jumbo-1-unstable-2026-07-07/bin/rar2john
";

    #[test]
    fn completes_and_dedups_executable_names() {
        let candidates =
            with_fake_nix_locate(SAMPLE_OUTPUT, || complete_command(OsStr::new("rar")));

        let mut values: Vec<String> = candidates
            .iter()
            .map(|c| c.get_value().to_string_lossy().into_owned())
            .collect();
        values.sort();

        assert_eq!(values, vec!["rar2hashcat", "rar2john", "rare", "rars"]);
    }

    #[test]
    fn non_utf8_input_yields_no_candidates() {
        use std::os::unix::ffi::OsStrExt;
        let invalid = OsStr::from_bytes(&[0xff, 0xfe]);
        assert!(complete_command(invalid).is_empty());
    }
}
