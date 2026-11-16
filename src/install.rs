use std::{env, path::PathBuf, process::Command};

use log::{debug, trace};

use crate::{
    nixpkgs::Nixpkgs,
    tools::{NIX, NIX_ENV},
};

/// The attribute of the package that `derivation` is from, and its output
/// that has the command, when that one has to be asked for. `derivation` is
/// how nix-locate names a package: its attribute, followed by that output,
/// e.g. `python3Packages.black.out`.
///
/// Installing a package of nixpkgs gives its output `bin`, or `out` if it
/// doesn't have that one, and the man pages. A command in another output,
/// like `curl-config` in `curl.dev`, is only there when the output is asked
/// for
fn package_and_output(derivation: &str) -> (&str, Option<&str>) {
    match derivation.rsplit_once('.') {
        Some((attr, "out" | "bin")) => (attr, None),
        Some((attr, output)) => (attr, Some(output)),
        None => (derivation, None),
    }
}

/// `s` as a string of the nix language
fn nix_string(s: &str) -> String {
    let escaped = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace("${", "\\${");
    format!("\"{escaped}\"")
}

/// The expression for `nix-env -i -E` that is the package `attr` of nixpkgs
/// with `output` added to the outputs that are installed of it.
///
/// `nix-env -iA curl.dev` would install that output alone. But the profile
/// then says of curl that the default outputs are the ones to install, which
/// aren't there, and nix-env fails on every install after it with "this
/// derivation has bad 'meta.outputsToInstall'"
fn nix_env_expr(attr: &str, output: &str) -> String {
    let path: String = attr
        .split('.')
        .map(|name| format!(".{}", nix_string(name)))
        .collect();
    let output = nix_string(output);

    format!(
        "nixpkgs: let package = (nixpkgs {{ }}){path}; in package // {{ \
         meta = package.meta // {{ outputsToInstall = \
         (package.meta.outputsToInstall or package.outputs) ++ [ {output} ]; }}; }}"
    )
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
        let refused = match Command::new(NIX_ENV).arg("--query").output() {
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

    /// The command that installs the package of `nixpkgs` that nix-locate
    /// calls `derivation`
    pub fn command(&self, nixpkgs: &Nixpkgs, derivation: &str) -> Command {
        let (attr, output) = package_and_output(derivation);

        let install_cmd = match self {
            Self::NixEnv => {
                let mut install_cmd = Command::new(NIX_ENV);

                install_cmd.args(nixpkgs.nix_env_args());
                match output {
                    Some(output) => install_cmd.args(["-i", "-E", &nix_env_expr(attr, output)]),
                    None => install_cmd.args(["-iA", attr]),
                };

                install_cmd
            }
            Self::NixProfile { profile } => {
                let mut install_cmd = Command::new(NIX);

                // `install` is called `add` since nix 2.30, which warns that
                // the old name is deprecated. Older versions only know
                // `install`
                install_cmd.args([
                    "--extra-experimental-features",
                    "nix-command flakes",
                    "profile",
                    "install",
                ]);
                // The output alone: which ones are installed by default
                // isn't known here. `attr.output` works too, but then the
                // name of the package in the profile is the one of the output
                let installable = match output {
                    Some(output) => format!("{attr}^{output}"),
                    None => attr.to_owned(),
                };
                if let Some(profile) = profile {
                    install_cmd.arg("--profile").arg(profile);
                }
                install_cmd.args(nixpkgs.profile_installable_args(&installable));

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
    fn packages_and_outputs() {
        assert_eq!(package_and_output("cowsay.out"), ("cowsay", None));
        assert_eq!(package_and_output("xz.bin"), ("xz", None));
        // Only the output is left out, not the attribute set of the package
        assert_eq!(
            package_and_output("python3Packages.black.out"),
            ("python3Packages.black", None)
        );
        // Not an output that is installed by default
        assert_eq!(package_and_output("curl.dev"), ("curl", Some("dev")));
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
            command_line(&nix_env.command(&nixpkgs, "cowsay.out")),
            [NIX_ENV, "-f", "<nixpkgs>", "-iA", "cowsay"]
        );
        assert_eq!(
            command_line(&nix_env.command(&nixpkgs, "python3Packages.libxml2.dev")),
            [
                NIX_ENV,
                "-f",
                "<nixpkgs>",
                "-i",
                "-E",
                "nixpkgs: let package = (nixpkgs { }).\"python3Packages\".\"libxml2\"; \
                 in package // { meta = package.meta // { outputsToInstall = \
                 (package.meta.outputsToInstall or package.outputs) ++ [ \"dev\" ]; }; }"
            ]
        );
        // nix-env doesn't take a flake, but its source
        assert_eq!(
            command_line(&nix_env.command(&unstable, "cowsay.out")),
            [
                NIX_ENV,
                "--extra-experimental-features",
                "flakes",
                "-f",
                "flake:github:nixos/nixpkgs/nixos-unstable",
                "-iA",
                "cowsay"
            ]
        );
        assert_eq!(
            command_line(&nix_profile.command(&unstable, "cowsay.out")),
            [
                NIX,
                "--extra-experimental-features",
                "nix-command flakes",
                "profile",
                "install",
                "--profile",
                "/my/profile",
                "github:nixos/nixpkgs/nixos-unstable#cowsay"
            ]
        );
        assert_eq!(
            command_line(&nix_profile.command(&unstable, "curl.dev")).last(),
            Some(&"github:nixos/nixpkgs/nixos-unstable#curl^dev")
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
