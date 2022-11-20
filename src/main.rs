mod index;
use std::{
    env,
    io::Write,
    os::unix::prelude::CommandExt,
    process::{Command, ExitCode, Stdio},
};

use clap::crate_version;
use clap::Parser;

fn pick(picker: &str, derivations: &[&str]) -> Option<String> {
    let mut picker_process = Command::new(&picker)
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

fn run_command(use_channel: bool, choice: &str, command: &str, trail: &[String]) {
    let mut run_cmd = Command::new("nix");

    run_cmd.args([
        "--extra-experimental-features",
        "nix-command flakes",
        "shell",
    ]);

    if use_channel {
        run_cmd.args(["-f", "<nixpkgs>", choice]);
    } else {
        run_cmd.args([format!("nixpkgs#{}", choice)]);
    }

    run_cmd.args(["--command", command]);
    run_cmd.args(trail);
    run_cmd.exec();
}

fn main() -> ExitCode {
    let args = Opt::parse();

    if args.update {
        index::update_database();
    }

    // The command may not be given if `--update` was specified.
    if args.cmd.is_empty() {
        return ExitCode::SUCCESS;
    }

    let command = &args.cmd[0];
    let trail = &args.cmd[1..];

    index::check_database();

    let nix_locate_output = Command::new("nix-locate")
        .args(["--top-level", "--minimal", "--at-root", "--whole-name"])
        .arg(format!("/bin/{}", command))
        .output()
        .expect("failed to execute nix-locate");

    if !nix_locate_output.status.success() {
        match std::str::from_utf8(&nix_locate_output.stderr) {
            Ok(stderr) => eprintln!("nix-locate failed with: {}", stderr),
            Err(_) => eprint!("nix-locate failed"),
        }
        return ExitCode::FAILURE;
    }

    let attrs = nix_locate_output.stdout;

    if attrs.is_empty() {
        eprintln!("No executable `{}` found in nix-index database.", command);
        return ExitCode::FAILURE;
    }

    let attrs: Vec<_> = std::str::from_utf8(&attrs)
        .expect("fail")
        .trim()
        .split('\n')
        .collect();

    let choice = if attrs.len() > 1 {
        match pick(&args.picker, &attrs) {
            Some(x) => x,
            None => return ExitCode::FAILURE,
        }
    } else {
        attrs.first().unwrap().trim().to_owned()
    };

    let use_channel = match env::var("NIX_PATH") {
        Ok(val) => val,
        Err(_) => "".to_owned(),
    }
    .contains("nixpkgs");

    if args.install {
        Command::new("nix-env")
            .args(["-f", "<nixpkgs>", "-iA", choice.rsplit('.').last().unwrap()])
            .exec();
    } else {
        run_command(use_channel, &choice, command, trail);
    }

    ExitCode::SUCCESS
}

/// Runs programs without installing them
#[derive(Parser)]
#[clap(version = crate_version!(), trailing_var_arg = true)]
struct Opt {
    /// Install the derivation containing the executable
    #[clap(short, long)]
    install: bool,

    #[clap(long, env = "COMMA_PICKER", default_value = "fzy")]
    picker: String,

    /// Update nix-index database
    #[clap(short, long)]
    update: bool,

    /// Command to run
    #[clap(required_unless_present = "update", name = "cmd")]
    cmd: Vec<String>,
}
