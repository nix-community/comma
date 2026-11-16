use std::{
    collections::HashMap,
    io::Write,
    process::{Command, Stdio},
};

use crate::{
    cache::{Cache, CacheEntry},
    index::index_database,
};

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

/// What is known about the package of a command before the user is asked
pub enum Found {
    /// The choice that is remembered in the cache
    Remembered(CacheEntry),
    /// The packages in the index that provide the command
    InIndex(Box<[String]>),
}

/// Look `command` up in `cache` and, when it isn't remembered there, in the
/// index
pub fn find_command(cache: Option<&Cache>, command: &str) -> Option<Found> {
    match cache.and_then(|cache| cache.query(command)) {
        Some(entry) => Some(Found::Remembered(entry)),
        None => index_database(command).map(Found::InIndex),
    }
}

/// The package for `command` out of what was `found` for it, and whether the
/// user chose it in the picker just now, rather than it being the only one or
/// remembered in the cache. The picker is only shown when there are several
/// packages. A package from the index is remembered in `cache`
pub fn choose_entry(
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
pub fn find_entry(
    cache: Option<&mut Cache>,
    command: &str,
    picker: &str,
) -> Option<(CacheEntry, bool)> {
    let found = find_command(cache.as_deref(), command)?;
    choose_entry(cache, command, &found, picker)
}
