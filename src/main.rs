use std::{
    env,
    io::Write,
    os::unix::prelude::CommandExt,
    process::{exit, Command, Stdio},
};

use clap::{arg, Arg};

fn pick(picker: &str, derivations: Vec<&str>) -> String {
    let mut picker_process = Command::new(&picker)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to execute {}: {}", picker, err));

    let picker_stdin = picker_process.stdin.as_mut().unwrap();

    picker_stdin
        .write_all(derivations.join("\n").as_bytes())
        .expect("failure to write stdin");

    let output = picker_process.wait_with_output().unwrap().stdout;

    if output.is_empty() {
        exit(1)
    }
    std::str::from_utf8(&output)
        .expect("fail")
        .trim()
        .to_string()
}

fn run_command(use_channel: bool, choice: &str, command: &str, trail: Vec<&str>) {
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

fn main() {
    let matches = clap::Command::new("comma")
        .about("runs programs without installing them")
        .arg(
            Arg::new("install")
                .short('i')
                .long("install")
                .takes_value(false)
                .help("install the derivation containing the executable"),
        )
        .trailing_var_arg(true)
        .arg(arg!(<cmd> ... "command to run"))
        .get_matches();

    let picker = match env::var("COMMA_PICKER") {
        Ok(val) => val,
        Err(_) => "fzy".to_string(),
    };

    let install = matches.is_present("install");

    let mut trail: Vec<&str> = matches.values_of("cmd").unwrap().collect();
    let command: String = trail.remove(0).to_string();

    let attrs = Command::new("nix-locate")
        .args(["--top-level", "--minimal", "--at-root", "--whole-name"])
        .arg(format!("/bin/{}", command))
        .output()
        .expect("failed to execute nix-locate")
        .stdout;

    if attrs.is_empty() {
        eprintln!("no match");
        std::process::exit(1)
    }

    let attrs: Vec<&str> = std::str::from_utf8(&attrs)
        .expect("fail")
        .trim()
        .split('\n')
        .collect();

    let choice = if attrs.len() != 1 {
        pick(&picker, attrs)
    } else {
        attrs.first().unwrap().trim().to_string()
    };

    let use_channel = (match env::var("NIX_PATH") {
        Ok(val) => val,
        Err(_) => "".to_string(),
    })
    .contains("nixpkgs");

    if install {
        Command::new("nix-env")
            .args(["-f", "<nixpkgs>", "-iA", choice.rsplit('.').last().unwrap()])
            .exec();
    } else {
        run_command(use_channel, &choice, &command, trail)
    }
}
