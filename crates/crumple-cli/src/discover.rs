//! Input discovery and output path mapping.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Found {
    pub path: PathBuf,
    /// Path relative to the input root (a file input's root is its parent).
    pub rel: PathBuf,
    pub supported: bool,
    /// Set when the input could not be read at all.
    pub error: Option<String>,
    /// Set for a symlink named directly on the command line (never followed).
    pub symlink: bool,
}

pub(crate) fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(OsStr::to_str)
        .map(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "png" | "jpg" | "jpeg" | "webp"
            )
        })
        .unwrap_or(false)
}

/// Recursively lists every input, following no symlinks, sorted by path.
///
/// `skip_dir` is the output directory, canonicalized. When it lies inside an input
/// folder it is not descended into, so a second run does not pick up its own outputs.
pub(crate) fn discover(inputs: &[PathBuf], skip_dir: Option<&Path>) -> Vec<Found> {
    let mut out = Vec::new();
    for input in inputs {
        let name = PathBuf::from(input.file_name().unwrap_or(input.as_os_str()));
        match std::fs::symlink_metadata(input) {
            Err(e) => out.push(Found {
                error: Some(format!("cannot read input: {e}")),
                ..found(input.clone(), name)
            }),
            Ok(m) if m.file_type().is_symlink() => out.push(Found {
                symlink: true,
                ..found(input.clone(), name)
            }),
            Ok(m) if m.is_dir() => walk(input, input, skip_dir, &mut out),
            Ok(_) => out.push(found(input.clone(), name)),
        }
    }
    out.sort_by(|a, b| a.path.cmp(&b.path));
    out.dedup_by(|a, b| a.path == b.path);
    out
}

fn found(path: PathBuf, rel: PathBuf) -> Found {
    let supported = is_supported(&path);
    Found {
        path,
        rel,
        supported,
        error: None,
        symlink: false,
    }
}

fn walk(root: &Path, dir: &Path, skip_dir: Option<&Path>, out: &mut Vec<Found>) {
    let rel = |p: &Path| p.strip_prefix(root).unwrap_or(p).to_path_buf();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            out.push(Found {
                supported: false,
                error: Some(format!("cannot read directory: {e}")),
                ..found(dir.to_path_buf(), rel(dir))
            });
            return;
        }
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        let path = entry.path();
        if ft.is_symlink() {
            continue;
        } else if ft.is_dir() {
            let is_out =
                skip_dir.is_some_and(|s| std::fs::canonicalize(&path).is_ok_and(|c| c == s));
            if !is_out {
                walk(root, &path, skip_dir, out);
            }
        } else if ft.is_file() {
            out.push(found(path.clone(), rel(&path)));
        }
    }
}

/// Case-folded form of a relative path, for comparing output names. Folding on every
/// OS keeps output names identical across platforms, and NTFS and APFS (by default)
/// treat `A.avif` and `a.avif` as the same file.
pub(crate) fn fold(p: &Path) -> String {
    p.to_string_lossy().to_lowercase()
}

/// Folded relative paths without extension that more than one supported input maps
/// to (`a.png` + `a.JPG`). Inputs in such a group use `<stem>.<orig-ext>.<new-ext>`.
pub(crate) fn collisions(found: &[Found]) -> HashSet<String> {
    let mut seen = HashSet::new();
    let mut dup = HashSet::new();
    for f in found.iter().filter(|f| f.supported && f.error.is_none()) {
        let key = fold(&f.rel.with_extension(""));
        if !seen.insert(key.clone()) {
            dup.insert(key);
        }
    }
    dup
}

/// Output path for `rel` under `out`. `new_ext` is `None` for a kept original,
/// which keeps its own name. Errors if the result is the input file itself.
pub(crate) fn output_path(
    out: &Path,
    input: &Path,
    rel: &Path,
    new_ext: Option<&str>,
    collides: bool,
) -> Result<PathBuf, String> {
    let target = match new_ext {
        None => out.join(rel),
        Some(ext) if collides => {
            let mut name = rel.file_name().unwrap_or_default().to_os_string();
            name.push(".");
            name.push(ext);
            out.join(rel.with_file_name(name))
        }
        Some(ext) => out.join(rel.with_extension(ext)),
    };
    if same_path(&target, input) {
        return Err("output path equals input path".to_string());
    }
    Ok(target)
}

/// True if `a` and `b` name the same file: lexically (case-insensitively on Windows),
/// or, when both exist, by identity, which also catches `..`, symlinked or junctioned
/// directories and hard links that a lexical check misses.
pub(crate) fn same_path(a: &Path, b: &Path) -> bool {
    let abs = |p: &Path| std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    let (la, lb) = (abs(a), abs(b));
    let lexical = if cfg!(windows) {
        la.to_string_lossy().to_lowercase() == lb.to_string_lossy().to_lowercase()
    } else {
        la == lb
    };
    lexical || same_file(a, b)
}

#[cfg(unix)]
fn same_file(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(x), Ok(y)) => x.dev() == y.dev() && x.ino() == y.ino(),
        _ => false,
    }
}

#[cfg(not(unix))]
fn same_file(a: &Path, b: &Path) -> bool {
    // On Windows, canonicalize resolves `..`, junctions, symlinks and 8.3 names and
    // returns the on-disk case.
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_extension_and_keeps_original() {
        let out = Path::new("out");
        let input = Path::new("in/a/b.png");
        let rel = Path::new("a/b.png");
        let p = output_path(out, input, rel, Some("avif"), false).unwrap();
        assert_eq!(p, Path::new("out/a/b.avif"));
        let p = output_path(out, input, rel, None, false).unwrap();
        assert_eq!(p, Path::new("out/a/b.png"));
    }

    #[test]
    fn collision_rule() {
        let fs = vec![
            found(PathBuf::from("in/a.png"), PathBuf::from("a.png")),
            found(PathBuf::from("in/a.jpg"), PathBuf::from("a.jpg")),
            found(PathBuf::from("in/b.png"), PathBuf::from("b.png")),
        ];
        let c = collisions(&fs);
        assert!(c.contains("a"));
        assert!(!c.contains("b"));
        let out = Path::new("out");
        let a_png = output_path(
            out,
            Path::new("in/a.png"),
            Path::new("a.png"),
            Some("avif"),
            true,
        );
        assert_eq!(a_png.unwrap(), Path::new("out/a.png.avif"));
        let a_jpg = output_path(
            out,
            Path::new("in/a.jpg"),
            Path::new("a.jpg"),
            Some("avif"),
            true,
        );
        assert_eq!(a_jpg.unwrap(), Path::new("out/a.jpg.avif"));
    }

    #[test]
    fn collision_ignores_case() {
        // `Photo.png` -> `Photo.avif` and `photo.JPG` -> `photo.avif` are one file on
        // NTFS and APFS, so they must use the long form too.
        let fs = vec![
            found(PathBuf::from("in/Photo.png"), PathBuf::from("Photo.png")),
            found(PathBuf::from("in/photo.JPG"), PathBuf::from("photo.JPG")),
            found(PathBuf::from("in/x.txt"), PathBuf::from("x.txt")),
            found(PathBuf::from("in/X.png"), PathBuf::from("X.png")),
        ];
        let c = collisions(&fs);
        assert!(c.contains("photo"), "{c:?}");
        assert!(!c.contains("x"), "unsupported inputs never collide: {c:?}");
    }

    #[test]
    fn refuses_output_equal_to_input() {
        let r = output_path(
            Path::new("in"),
            Path::new("in/x.png"),
            Path::new("x.png"),
            None,
            false,
        );
        assert!(r.is_err());
        let r = output_path(
            Path::new("in"),
            Path::new("in/x.webp"),
            Path::new("x.png"),
            Some("webp"),
            false,
        );
        assert!(r.is_err());
    }

    #[test]
    fn refuses_output_equal_to_input_through_dotdot() {
        let dir = tempfile::tempdir().unwrap();
        let input_dir = dir.path().join("in");
        std::fs::create_dir_all(input_dir.join("sub")).unwrap();
        let input = input_dir.join("x.png");
        std::fs::write(&input, b"x").unwrap();
        // `in/sub/..` is `in`: lexically different on Unix, the same directory on disk.
        let out = input_dir.join("sub").join("..");
        let r = output_path(&out, &input, Path::new("x.png"), None, false);
        assert!(r.is_err(), "{r:?}");
    }

    #[test]
    fn discovery_filters_and_sorts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("sub/deeper")).unwrap();
        for f in [
            "z.PNG",
            "b.jpeg",
            "sub/a.JPG",
            "sub/deeper/c.webp",
            "notes.txt",
            "sub/x.gif",
        ] {
            std::fs::write(root.join(f), b"x").unwrap();
        }
        let got = discover(&[root.to_path_buf()], None);
        let mut sorted = got.clone();
        sorted.sort_by(|a, b| a.path.cmp(&b.path));
        assert_eq!(got, sorted);
        let rels: Vec<(String, bool)> = got
            .iter()
            .map(|f| (f.rel.to_string_lossy().replace('\\', "/"), f.supported))
            .collect();
        assert_eq!(rels.len(), 6);
        for (r, s) in &rels {
            let want = !(r.ends_with(".txt") || r.ends_with(".gif"));
            assert_eq!(*s, want, "{r}");
        }
        assert!(rels.iter().any(|(r, _)| r == "sub/deeper/c.webp"));
    }

    #[test]
    fn discovery_skips_output_dir_inside_input() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("out/nested")).unwrap();
        std::fs::write(root.join("a.png"), b"x").unwrap();
        std::fs::write(root.join("out/a.png"), b"x").unwrap();
        std::fs::write(root.join("out/nested/b.png"), b"x").unwrap();
        let skip = std::fs::canonicalize(root.join("out")).unwrap();
        let got = discover(&[root.to_path_buf()], Some(&skip));
        let rels: Vec<_> = got.iter().map(|f| f.rel.clone()).collect();
        assert_eq!(rels, vec![PathBuf::from("a.png")]);
        // Without a skip dir the outputs would be inputs again.
        assert_eq!(discover(&[root.to_path_buf()], None).len(), 3);
    }
}
