use std::process::Command;

use log::trace;

use crate::Nixpkgs;

/// The attribute of the package that `derivation` is from. That is how
/// nix-locate names a package: its attribute, followed by the output that has
/// the command, e.g. `python3Packages.black.out`
pub fn package_attr(derivation: &str) -> &str {
    derivation
        .rsplit_once('.')
        .map_or(derivation, |(attr, _output)| attr)
}

/// `nix-env`, installing the package `attr` of `nixpkgs`
pub fn install_command(nixpkgs: &Nixpkgs, attr: &str) -> Command {
    let mut install_cmd = Command::new("nix-env");

    install_cmd.args(nixpkgs.nix_env_args());
    install_cmd.args(["-iA", attr]);

    trace!("install command arguments: {install_cmd:?}");

    install_cmd
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The program and the arguments of `cmd`
    fn command_line(cmd: &Command) -> Vec<&str> {
        std::iter::once(cmd.get_program())
            .chain(cmd.get_args())
            .map(|arg| arg.to_str().unwrap())
            .collect()
    }

    #[test]
    fn package_attrs() {
        assert_eq!(package_attr("cowsay.out"), "cowsay");
        // Only the output is left out, not the attribute set of the package
        assert_eq!(
            package_attr("python3Packages.black.out"),
            "python3Packages.black"
        );
    }

    #[test]
    fn install_commands() {
        let nixpkgs = Nixpkgs::Channel {
            nix_path: "nixpkgs=/nix/nixpkgs".to_owned(),
        };
        let unstable = Nixpkgs::Flake("github:nixos/nixpkgs/nixos-unstable".to_owned());

        assert_eq!(
            command_line(&install_command(&nixpkgs, "cowsay")),
            ["nix-env", "-f", "<nixpkgs>", "-iA", "cowsay"]
        );
        // nix-env doesn't take a flake, but its source
        assert_eq!(
            command_line(&install_command(&unstable, "cowsay")),
            [
                "nix-env",
                "--extra-experimental-features",
                "flakes",
                "-f",
                "flake:github:nixos/nixpkgs/nixos-unstable",
                "-iA",
                "cowsay"
            ]
        );
    }
}
