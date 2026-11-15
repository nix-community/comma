/// The attribute of the package that `derivation` is from. That is how
/// nix-locate names a package: its attribute, followed by the output that has
/// the command, e.g. `python3Packages.black.out`
pub fn package_attr(derivation: &str) -> &str {
    derivation
        .rsplit_once('.')
        .map_or(derivation, |(attr, _output)| attr)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_attrs() {
        assert_eq!(package_attr("cowsay.out"), "cowsay");
        // Only the output is left out, not the attribute set of the package
        assert_eq!(
            package_attr("python3Packages.black.out"),
            "python3Packages.black"
        );
    }
}
