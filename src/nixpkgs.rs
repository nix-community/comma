use std::env;

/// The nixpkgs that packages are built from
#[derive(Debug, PartialEq)]
pub enum Nixpkgs {
    /// `<nixpkgs>`, which nix resolves through the given `NIX_PATH`
    Channel { nix_path: String },
    /// A flake reference, e.g. `nixpkgs` or `github:nixos/nixpkgs/nixos-unstable`
    Flake(String),
}

impl Nixpkgs {
    /// An explicitly given flake (via -F or `COMMA_NIXPKGS_FLAKE`) takes
    /// precedence over `<nixpkgs>` from `NIX_PATH`
    pub fn from_args(explicit_flake: Option<&str>) -> Self {
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
    pub fn installable_args(&self, attrs: &[&str]) -> Vec<String> {
        match self {
            Self::Channel { .. } => ["-f", "<nixpkgs>"]
                .iter()
                .chain(attrs)
                .map(|&arg| arg.to_owned())
                .collect(),
            Self::Flake(flake) => attrs.iter().map(|attr| format!("{flake}#{attr}")).collect(),
        }
    }

    /// The arguments that make `nix profile` install `installable` from this
    /// nixpkgs. Only for a flake it records where the package is from, which
    /// `nix profile upgrade` needs. So it is given as one whenever it is one:
    /// also `<nixpkgs>`, when its entry in `NIX_PATH` is `flake:...`
    pub fn profile_installable_args(&self, installable: &str) -> Vec<String> {
        let flake = match self {
            Self::Channel { nix_path } => nixpkgs_entry(nix_path)
                .and_then(|value| value.strip_prefix("flake:"))
                .filter(|flake| !flake.is_empty()),
            Self::Flake(flake) => Some(flake.as_str()),
        };

        match flake {
            Some(flake) => vec![format!("{flake}#{installable}")],
            None => self.installable_args(&[installable]),
        }
    }

    /// The arguments that make `nix-env` use this nixpkgs. It doesn't take a
    /// flake, but `-f flake:...` is the source of one, which for nixpkgs is
    /// the same as `<nixpkgs>` of a channel
    pub fn nix_env_args(&self) -> Vec<String> {
        match self {
            Self::Channel { .. } => vec!["-f".to_owned(), "<nixpkgs>".to_owned()],
            Self::Flake(flake) => vec![
                "--extra-experimental-features".to_owned(),
                "flakes".to_owned(),
                "-f".to_owned(),
                format!("flake:{flake}"),
            ],
        }
    }

    /// The key that paths built from this nixpkgs are cached under.
    /// `<nixpkgs>` is resolved through `NIX_PATH`, so the key contains the
    /// entry that provides it. Other entries don't matter: changing
    /// `nixos-config=` shouldn't make comma evaluate every command again
    pub fn cache_key(&self) -> String {
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
    fn nixpkgs_profile_installable_args() {
        // `<nixpkgs>` that is a flake is installed from as one
        assert_eq!(
            channel("nixos-config=/etc/nixos:nixpkgs=flake:nixpkgs")
                .profile_installable_args("cowsay"),
            ["nixpkgs#cowsay"]
        );
        assert_eq!(
            channel("nixpkgs=/nix/nixpkgs").profile_installable_args("cowsay"),
            ["-f", "<nixpkgs>", "cowsay"]
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
