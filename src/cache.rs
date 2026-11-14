use std::{
    collections::HashMap,
    env,
    error::Error,
    fs, io,
    path::{Path, PathBuf},
    process,
};

use bitcode::{Decode, Encode};
use log::{debug, error};

/// Marks a file as a comma cache. The cache file is this, followed by
/// `FORMAT_VERSION` as a little-endian u32, followed by the bitcode encoded
/// `CacheData`. Being a plain prefix, it can be checked without depending on
/// how bitcode lays out the rest
const MAGIC: &[u8] = b"comma-cache\0";

/// The version of the data that follows the header. Increase it whenever
/// `CacheData` changes, so that other versions of comma can tell that they
/// can't read the file
const FORMAT_VERSION: u32 = 1;

#[derive(Encode, Decode, Debug, Default, PartialEq)]
struct CacheData(HashMap<String, CacheEntry>);

/// Why the contents of a cache file can't be used
#[derive(Debug, PartialEq)]
enum DecodeError {
    /// The file doesn't start with `MAGIC`. Versions of comma from before the
    /// format was versioned wrote only the data
    NoHeader,
    /// The file was written with another `FORMAT_VERSION`
    Version(u32),
    /// The header is cut short, or it is fine but the data doesn't decode
    Corrupt(String),
}

fn encode(data: &CacheData) -> Vec<u8> {
    let mut bytes = MAGIC.to_vec();
    bytes.extend(FORMAT_VERSION.to_le_bytes());
    bytes.extend(bitcode::encode(data));
    bytes
}

fn decode(bytes: &[u8]) -> Result<CacheData, DecodeError> {
    let Some(rest) = bytes.strip_prefix(MAGIC) else {
        return Err(if MAGIC.starts_with(bytes) {
            DecodeError::Corrupt("the file ends inside the header".to_owned())
        } else {
            DecodeError::NoHeader
        });
    };
    let Some((version, data)) = rest.split_first_chunk() else {
        return Err(DecodeError::Corrupt(
            "the file ends inside the header".to_owned(),
        ));
    };
    let version = u32::from_le_bytes(*version);
    if version != FORMAT_VERSION {
        return Err(DecodeError::Version(version));
    }
    bitcode::decode(data).map_err(|e| DecodeError::Corrupt(e.to_string()))
}

#[derive(Encode, Decode, Clone, Debug, PartialEq)]
pub struct CacheEntry {
    pub derivation: String,
    /// Evaluated command paths, keyed by the nixpkgs source (flake ref, or
    /// `<nixpkgs>` with the NIX_PATH entry it was resolved through) they were
    /// built from
    pub paths: HashMap<String, String>,
}

impl CacheEntry {
    /// Remember `path` as the command path when built from `source`
    pub fn set_path(&mut self, source: &str, path: String) {
        // Nothing else removes the paths of sources that aren't used anymore,
        // so drop the ones that were garbage collected
        self.paths.retain(|_, path| Path::new(path).exists());
        self.paths.insert(source.to_owned(), path);
    }
}

/// The path with the home directory replaced by `~`, so that messages
/// shown to the user don't expose their home directory
fn display_path(path: &Path) -> String {
    env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .and_then(|home| path.strip_prefix(home).ok())
        .map_or_else(
            || path.display().to_string(),
            |rest| format!("~/{}", rest.display()),
        )
}

pub struct Cache {
    path: PathBuf,
    data: CacheData,
    /// The changes made since the file was read, by command: the new entry,
    /// or `None` for a deleted one
    changes: HashMap<String, Option<CacheEntry>>,
    /// Whether the cache was emptied before `changes` were made
    emptied: bool,
}

/// The end of every message about a cache that can't be used
const CHOICES_LOST: &str = "comma: remembered package choices and cached paths are lost, \
                            so you will be asked to pick packages again.";

impl Cache {
    pub fn new() -> Result<Self, Box<dyn Error>> {
        Self::load(xdg::BaseDirectories::new()?.place_state_file("comma/choices")?)
    }

    /// Read the cache file at `path`. A file that can't be used gives an
    /// empty cache and a message, which is shown by default since the user's
    /// package choices are lost
    fn load(path: PathBuf) -> Result<Self, Box<dyn Error>> {
        debug!("creating new cache instance for path: {}", path.display());

        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e.into()),
        };

        let data = if bytes.is_empty() {
            // Nothing in it that could be lost or would be worth keeping
            CacheData::default()
        } else {
            decode(&bytes).or_else(|e| Self::unusable(&path, e))?
        };

        Ok(Self {
            path,
            data,
            changes: HashMap::new(),
            emptied: false,
        })
    }

    /// Tell the user why the cache file at `path` isn't used, and return the
    /// empty cache to continue with
    fn unusable(path: &Path, error: DecodeError) -> Result<CacheData, Box<dyn Error>> {
        let shown = display_path(path);
        match error {
            // The next update overwrites the file
            DecodeError::NoHeader => eprintln!(
                "comma: discarding the cache at '{shown}' because it has no \
                 format version\n\
                 comma: the cache format may have changed between comma versions.\n\
                 {CHOICES_LOST}"
            ),
            // As above
            DecodeError::Version(version) => eprintln!(
                "comma: the cache at '{shown}' was written by a different \
                 version of comma (format {version}, expected {FORMAT_VERSION}), \
                 starting with an empty cache.\n\
                 {CHOICES_LOST}"
            ),
            // This isn't a file that comma would write, so keep it around
            // instead of overwriting it
            DecodeError::Corrupt(reason) => {
                let corrupt = path.with_extension("corrupt");
                fs::rename(path, &corrupt).map_err(|e| {
                    format!(
                        "the cache at '{shown}' is corrupt ({reason}) and could \
                         not be moved to '{}': {e}",
                        display_path(&corrupt)
                    )
                })?;
                eprintln!(
                    "comma: the cache at '{shown}' is corrupt ({reason}). It was \
                     moved to '{}', starting with an empty cache.\n\
                     {CHOICES_LOST}",
                    display_path(&corrupt)
                );
            }
        }
        Ok(CacheData::default())
    }

    /// Write the changes to the cache file, in a way that works with another
    /// comma running at the same time (`, jq . | , fx`)
    fn write(&self) -> io::Result<()> {
        // Apply the changes to what is in the file now, not to what was read
        // at startup, so that entries the other comma has written since then
        // are kept. A file that can't be used is replaced, as `load` announced
        let mut data = match fs::read(&self.path).map(|bytes| decode(&bytes)) {
            Ok(Ok(data)) if !self.emptied => data,
            _ => CacheData::default(),
        };
        for (command, entry) in &self.changes {
            match entry {
                Some(entry) => data.0.insert(command.clone(), entry.clone()),
                None => data.0.remove(command),
            };
        }

        // Write a temporary file and rename it over the cache file, so that
        // the other comma reads either the old or the new contents, never a
        // partially written file. It is named after the process, so that two
        // commas don't write to the same temporary file
        let mut tmp = self.path.clone().into_os_string();
        tmp.push(format!(".{}.tmp", process::id()));
        let tmp = PathBuf::from(tmp);

        fs::write(&tmp, encode(&data))
            .and_then(|()| fs::rename(&tmp, &self.path))
            .inspect_err(|_| {
                let _ = fs::remove_file(&tmp);
            })
    }

    pub fn query(&self, command: &str) -> Option<CacheEntry> {
        debug!("querying cache entry for command '{command}'");
        self.data.0.get(command).cloned()
    }

    /// All cache entries, sorted by command name
    pub fn entries(&self) -> Vec<(&String, &CacheEntry)> {
        let mut entries: Vec<_> = self.data.0.iter().collect();
        entries.sort_by_key(|(command, _)| *command);
        entries
    }

    pub fn update(&mut self, command: &str, entry: CacheEntry) {
        debug!("updating cache entry for command '{command}': {entry:?}");
        self.data.0.insert(command.into(), entry.clone());
        self.changes.insert(command.into(), Some(entry));
    }

    pub fn delete(&mut self, command: &str) {
        debug!("deleting cache for command '{command}'");
        self.data.0.remove(command);
        self.changes.insert(command.into(), None);
    }

    pub fn empty(&mut self) {
        debug!("emptying cache");
        self.data.0.clear();
        self.changes.clear();
        self.emptied = true;
    }
}

impl Drop for Cache {
    fn drop(&mut self) {
        if self.emptied || !self.changes.is_empty() {
            debug!("writing cache to disk: {}", self.path.display());
            if let Err(e) = self.write() {
                error!(
                    "failed to write cache to '{}': {e}",
                    display_path(&self.path)
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A new, empty directory for the test called `name`
    fn test_dir(name: &str) -> PathBuf {
        let dir = env::temp_dir().join(format!("comma-test-{}-{name}", process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(derivation: &str) -> CacheEntry {
        CacheEntry {
            derivation: derivation.to_owned(),
            paths: HashMap::new(),
        }
    }

    fn data() -> CacheData {
        CacheData(HashMap::from([("hello".to_owned(), entry("hello.out"))]))
    }

    fn with_version(version: u32) -> Vec<u8> {
        let mut bytes = MAGIC.to_vec();
        bytes.extend(version.to_le_bytes());
        bytes.extend(bitcode::encode(&data()));
        bytes
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn set_path_drops_garbage_collected_paths() {
        let dir = test_dir("set_path_drops_garbage_collected_paths");
        let existing = dir.join("existing").display().to_string();
        let gone = dir.join("gone").display().to_string();
        let new = dir.join("new").display().to_string();
        fs::write(&existing, b"").unwrap();

        let mut entry = entry("hello.out");
        entry.paths.insert("nixpkgs".to_owned(), existing.clone());
        entry.paths.insert("<nixpkgs> (/old)".to_owned(), gone);
        // The new path is stored without being checked
        entry.set_path("<nixpkgs> (/new)", new.clone());

        assert_eq!(
            entry.paths,
            HashMap::from([
                ("nixpkgs".to_owned(), existing),
                ("<nixpkgs> (/new)".to_owned(), new),
            ])
        );

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn decode_unusable() {
        // What versions of comma before the header existed wrote
        assert_eq!(
            decode(&bitcode::encode(&data())),
            Err(DecodeError::NoHeader)
        );

        assert_eq!(
            decode(&with_version(FORMAT_VERSION + 1)),
            Err(DecodeError::Version(FORMAT_VERSION + 1))
        );

        // Cut short inside the magic and inside the version. For the data,
        // see `load_corrupt_file`
        let valid = encode(&data());
        for len in [3, MAGIC.len() + 3] {
            assert!(
                matches!(decode(&valid[..len]), Err(DecodeError::Corrupt(_))),
                "{len} bytes"
            );
        }
    }

    #[test]
    fn load_empty_file() {
        let dir = test_dir("load_empty_file");
        let path = dir.join("choices");
        fs::write(&path, b"").unwrap();

        // An empty cache, not a corrupt file that is moved away
        assert!(Cache::load(path).unwrap().entries().is_empty());
        assert_eq!(file_names(&dir), ["choices"]);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn concurrent_changes_are_merged() {
        let dir = test_dir("concurrent_changes_are_merged");
        let path = dir.join("choices");

        let mut cache = Cache::load(path.clone()).unwrap();
        cache.update("vi", entry("vim.out"));
        cache.update("hello", entry("hello.out"));
        drop(cache);

        // Two commas that both read the file before either of them writes it
        let mut first = Cache::load(path.clone()).unwrap();
        let mut second = Cache::load(path.clone()).unwrap();
        first.update("jq", entry("jq.bin"));
        first.delete("hello");
        second.update("fx", entry("fx.out"));
        drop(first);
        drop(second);

        let cache = Cache::load(path.clone()).unwrap();
        let commands: Vec<_> = cache.entries().into_iter().map(|(c, _)| c).collect();
        assert_eq!(commands, ["fx", "jq", "vi"]);

        // Emptying the cache also removes what was written in the meantime
        let mut first = Cache::load(path.clone()).unwrap();
        let mut second = Cache::load(path.clone()).unwrap();
        first.update("hello", entry("hello.out"));
        second.empty();
        second.update("rg", entry("ripgrep.out"));
        drop(first);
        drop(second);

        let cache = Cache::load(path).unwrap();
        let commands: Vec<_> = cache.entries().into_iter().map(|(c, _)| c).collect();
        assert_eq!(commands, ["rg"]);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_corrupt_file() {
        let dir = test_dir("load_corrupt_file");
        let path = dir.join("choices");
        let mut bytes = encode(&data());
        bytes.truncate(bytes.len() - 1);
        fs::write(&path, &bytes).unwrap();

        let cache = Cache::load(path).unwrap();
        assert!(cache.entries().is_empty());

        // The file is moved away instead of being overwritten
        assert_eq!(file_names(&dir), ["choices.corrupt"]);
        assert_eq!(fs::read(dir.join("choices.corrupt")).unwrap(), bytes);

        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn load_unversioned_or_other_version_file() {
        let dir = test_dir("load_unversioned_or_other_version_file");
        let path = dir.join("choices");

        for bytes in [b"garbage".to_vec(), with_version(FORMAT_VERSION + 1)] {
            fs::write(&path, &bytes).unwrap();

            // Starts empty and leaves the file alone until the next update
            let mut cache = Cache::load(path.clone()).unwrap();
            assert!(cache.entries().is_empty());
            assert_eq!(file_names(&dir), ["choices"]);
            assert_eq!(fs::read(&path).unwrap(), bytes);

            cache.update("hello", entry("hello.out"));
            drop(cache);
            assert_eq!(file_names(&dir), ["choices"]);
            assert_eq!(fs::read(&path).unwrap(), encode(&data()));
        }

        fs::remove_dir_all(dir).unwrap();
    }
}
