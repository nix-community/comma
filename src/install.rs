use std::{env, path::PathBuf, process::Command};

use log::{debug, trace};

use crate::Nixpkgs;

/// The attribute of the package that `derivation` is from. That is how
/// nix-locate names a package: its attribute, followed by the output that has
/// the command, e.g. `python3Packages.black.out`
pub fn package_attr(derivation: &str) -> &str {
    derivation
        .rsplit_once('.')
        .map_or(derivation, |(attr, _output)| attr)
}

/// Whether `stderr` of nix-env says that it refuses the profile, which it
/// does for one that `nix profile` manages:
///
/// error: profile "..." is incompatible with 'nix-env'; please use 'nix
/// profile' instead
fn refused_by_nix_env(stderr: &[u8]) -> bool {
    String::from_utf8_lossy(stderr).contains("is incompatible with 'nix-env'")
}

/// What installs a package into the profile of the user
#[derive(Debug, PartialEq)]
pub enum Installer {
    /// `nix-env`, which finds the profile by itself
    NixEnv,
    /// `nix profile`. `profile` is the one given in `NIX_PROFILE`, which
    /// only nix-env reads
    NixProfile { profile: Option<PathBuf> },
}

impl Installer {
    /// nix-env, unless it refuses the profile that it would install into.
    /// That is for nix to tell, so nix-env is asked: where the profile is
    /// depends on the environment and on the settings of nix.
    ///
    /// The other way around, `nix profile` does install into a profile of
    /// nix-env, and also lists what is in it. But it turns the profile into
    /// one of its own, which nix-env refuses from then on. So it isn't used
    /// for any other profile, a new one included, nor when nix-env fails in
    /// another way
    pub fn detect() -> Self {
        // Listing what is installed is the least that makes nix-env look at
        // the profile
        let refused = match Command::new("nix-env").arg("--query").output() {
            Ok(output) => !output.status.success() && refused_by_nix_env(&output.stderr),
            Err(err) => {
                debug!("failed to execute nix-env: {err}");
                false
            }
        };

        if refused {
            // Both find the default profile in the same way
            Self::NixProfile {
                profile: env::var_os("NIX_PROFILE")
                    .filter(|profile| !profile.is_empty())
                    .map(PathBuf::from),
            }
        } else {
            Self::NixEnv
        }
    }

    /// The command that installs the package `attr` of `nixpkgs`
    pub fn command(&self, nixpkgs: &Nixpkgs, attr: &str) -> Command {
        let install_cmd = match self {
            Self::NixEnv => {
                let mut install_cmd = Command::new("nix-env");

                install_cmd.args(nixpkgs.nix_env_args());
                install_cmd.args(["-iA", attr]);

                install_cmd
            }
            Self::NixProfile { profile } => {
                let mut install_cmd = Command::new("nix");

                // `install` is called `add` since nix 2.30, which warns that
                // the old name is deprecated. Older versions only know
                // `install`
                install_cmd.args([
                    "--extra-experimental-features",
                    "nix-command flakes",
                    "profile",
                    "install",
                ]);
                if let Some(profile) = profile {
                    install_cmd.arg("--profile").arg(profile);
                }
                install_cmd.args(nixpkgs.installable_args(&[attr]));

                install_cmd
            }
        };

        trace!("install command arguments: {install_cmd:?}");

        install_cmd
    }
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
        let nix_env = Installer::NixEnv;
        let nix_profile = Installer::NixProfile {
            profile: Some("/my/profile".into()),
        };
        let nixpkgs = Nixpkgs::Channel {
            nix_path: "nixpkgs=/nix/nixpkgs".to_owned(),
        };
        let unstable = Nixpkgs::Flake("github:nixos/nixpkgs/nixos-unstable".to_owned());

        assert_eq!(
            command_line(&nix_env.command(&nixpkgs, "cowsay")),
            ["nix-env", "-f", "<nixpkgs>", "-iA", "cowsay"]
        );
        // nix-env doesn't take a flake, but its source
        assert_eq!(
            command_line(&nix_env.command(&unstable, "cowsay")),
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
        assert_eq!(
            command_line(&nix_profile.command(&unstable, "cowsay")),
            [
                "nix",
                "--extra-experimental-features",
                "nix-command flakes",
                "profile",
                "install",
                "--profile",
                "/my/profile",
                "github:nixos/nixpkgs/nixos-unstable#cowsay"
            ]
        );
    }

    #[test]
    fn nix_env_refusals() {
        assert!(refused_by_nix_env(
            b"error: profile \"/home/me/.local/state/nix/profiles/profile\" is \
              incompatible with 'nix-env'; please use 'nix profile' instead\n"
        ));
        assert!(!refused_by_nix_env(
            b"error: opening lock file \"/nix/var/nix/profiles/default.lock\": Permission denied\n"
        ));
    }
}
