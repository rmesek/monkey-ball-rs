//! Content store: validate a user-supplied content folder and import it into app-private
//! storage (the mobile flow described in docs/GETTING_ASSETS.md). Platform shells do the
//! picking (Android SAF, iOS document picker, browser file input) and hand us a directory.

use crate::layout::{stage_files, GameSource};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Result of checking a content root against the expected layout.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct ContentReport {
    pub present: Vec<String>,
    pub missing: Vec<String>,
}

impl ContentReport {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty()
    }
}

/// Check that every file in `required` exists below `root`.
pub fn validate_files(root: &Path, required: &[String]) -> ContentReport {
    let mut report = ContentReport::default();
    for rel in required {
        if root.join(rel).is_file() {
            report.present.push(rel.clone());
        } else {
            report.missing.push(rel.clone());
        }
    }
    report
}

/// Check one stage (background name needed because stages map to backgrounds).
pub fn validate_stage(
    root: &Path,
    source: GameSource,
    stage_id: u32,
    bg_name: &str,
    naomi: bool,
) -> ContentReport {
    validate_files(root, &stage_files(source, stage_id, bg_name, naomi))
}

/// Where imported content lives and how it gets there.
pub trait ContentStore {
    /// Directory the game reads from (feed it to `DirPack`).
    fn root(&self) -> &Path;

    /// True once an import has completed.
    fn has_content(&self) -> bool;

    /// Copy `files` (paths relative to both roots) from a user-picked directory into the store.
    /// `progress(done, total)` is called after each file. Stops at the first error.
    fn import_from_dir(
        &self,
        picked: &Path,
        files: &[String],
        progress: &mut dyn FnMut(usize, usize),
    ) -> io::Result<()>;
}

/// Store backed by an app-private directory (Android `filesDir`, iOS Application Support, desktop data dir).
pub struct DirStore {
    root: PathBuf,
}

const COMPLETE_MARKER: &str = ".import_complete";

impl DirStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl ContentStore for DirStore {
    fn root(&self) -> &Path {
        &self.root
    }

    fn has_content(&self) -> bool {
        self.root.join(COMPLETE_MARKER).is_file()
    }

    fn import_from_dir(
        &self,
        picked: &Path,
        files: &[String],
        progress: &mut dyn FnMut(usize, usize),
    ) -> io::Result<()> {
        let _ = fs::remove_file(self.root.join(COMPLETE_MARKER));
        let total = files.len();
        for (i, rel) in files.iter().enumerate() {
            if rel.split(['/', '\\']).any(|c| c == "..") {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "path escapes root",
                ));
            }
            let dest = self.root.join(rel);
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::copy(picked.join(rel), &dest)?;
            progress(i + 1, total);
        }
        fs::create_dir_all(&self.root)?;
        fs::write(self.root.join(COMPLETE_MARKER), b"")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("mbrs-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn validates_and_imports() {
        let src = temp("src");
        let dst = temp("dst");
        let files = vec!["a/one.bin".to_string(), "b/two.bin".to_string()];
        fs::create_dir_all(src.join("a")).unwrap();
        fs::write(src.join("a/one.bin"), [1u8]).unwrap();

        let r = validate_files(&src, &files);
        assert_eq!(r.missing, vec!["b/two.bin".to_string()]);
        assert!(!r.is_complete());

        let store = DirStore::new(&dst);
        assert!(!store.has_content());
        let mut calls = 0;
        assert!(store
            .import_from_dir(&src, &files, &mut |_, _| calls += 1)
            .is_err());
        assert!(!store.has_content());

        fs::create_dir_all(src.join("b")).unwrap();
        fs::write(src.join("b/two.bin"), [2u8]).unwrap();
        store
            .import_from_dir(&src, &files, &mut |_, _| calls += 1)
            .unwrap();
        assert!(store.has_content());
        assert_eq!(fs::read(dst.join("b/two.bin")).unwrap(), vec![2u8]);

        let _ = fs::remove_dir_all(&src);
        let _ = fs::remove_dir_all(&dst);
    }

    #[test]
    fn rejects_traversal() {
        let dst = temp("trav");
        let store = DirStore::new(&dst);
        let r = store.import_from_dir(Path::new("."), &["../x".to_string()], &mut |_, _| {});
        assert!(r.is_err());
        let _ = fs::remove_dir_all(&dst);
    }
}
