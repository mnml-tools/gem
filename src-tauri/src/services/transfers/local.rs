//! The local side of transfers: safe local names for remote files, "keep both" numbering,
//! partial-download paths and walking local folders for uploads.
//!
//! Remote names are untrusted (plan §4.6): a server may send `..`, separators, control
//! characters or names Windows cannot store. Every component of a local download path goes
//! through [`sanitize_name`].

use std::path::{Path, PathBuf};

use crate::error::{AppError, AppResult};

/// Suffix of a download in progress; renamed into place only once complete.
pub const PART_SUFFIX: &str = ".gem-part";

const MAX_NAME_BYTES: usize = 255;

const WINDOWS_RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// A remote file name made safe as one local path component on this OS.
pub fn sanitize_name(name: &str) -> String {
    sanitize_name_for(name, cfg!(windows))
}

/// [`sanitize_name`] with the Windows rules switchable, so both are tested everywhere.
pub fn sanitize_name_for(name: &str, windows: bool) -> String {
    let mut out: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' => '_',
            c if c.is_control() => '_',
            '<' | '>' | ':' | '"' | '|' | '?' | '*' if windows => '_',
            c => c,
        })
        .collect();
    if windows {
        // Windows silently drops trailing dots and spaces, which could merge two names.
        out = out.trim_end_matches(['.', ' ']).to_owned();
        let stem = out.split('.').next().unwrap_or("").trim_end();
        if WINDOWS_RESERVED
            .iter()
            .any(|r| r.eq_ignore_ascii_case(stem))
        {
            out.insert(0, '_');
        }
    }
    if out.is_empty() || out == "." || out == ".." {
        out = "_".into();
    }
    if out.len() > MAX_NAME_BYTES {
        let mut cut = MAX_NAME_BYTES;
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out.truncate(cut);
    }
    out
}

/// `report.pdf` → `report (n).pdf`; dotfiles and names without an extension get the suffix
/// at the end (`.env (n)`).
pub fn numbered_name(name: &str, n: u32) -> String {
    match name.rfind('.') {
        Some(dot) if dot > 0 => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        _ => format!("{name} ({n})"),
    }
}

/// Where a download is written until it is complete: `<name>.gem-part` next to the target.
pub fn part_path(target: &Path) -> PathBuf {
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    target.with_file_name(format!("{name}{PART_SUFFIX}"))
}

/// One thing to upload from a local walk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalItem {
    pub path: PathBuf,
    /// Path components relative to the folder the upload goes into (`["site", "css", "a.css"]`).
    pub relative: Vec<String>,
    pub is_dir: bool,
    pub size: Option<u64>,
}

fn component(path: &Path) -> AppResult<String> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .ok_or_else(|| AppError::validation(format!("{} has no name", path.display())))?;
    if !crate::remote::path::is_valid_name(&name) {
        return Err(AppError::validation(format!(
            "{name} is not a valid remote file name"
        )));
    }
    Ok(name)
}

/// Expands files and folders chosen by the user into a flat list, directories before their
/// contents. The roots are followed if they are symlinks (the user picked them); inside
/// folders, links to files are uploaded as files and links to folders are skipped (no loops).
pub fn walk(roots: &[PathBuf]) -> AppResult<Vec<LocalItem>> {
    let mut items = Vec::new();
    for root in roots {
        let metadata = std::fs::metadata(root)
            .map_err(|e| AppError::Platform(format!("cannot read {}: {e}", root.display())))?;
        let name = component(root)?;
        if metadata.is_dir() {
            walk_dir(root, vec![name], &mut items)?;
        } else if metadata.is_file() {
            items.push(LocalItem {
                path: root.clone(),
                relative: vec![name],
                is_dir: false,
                size: Some(metadata.len()),
            });
        }
    }
    Ok(items)
}

fn walk_dir(dir: &Path, relative: Vec<String>, items: &mut Vec<LocalItem>) -> AppResult<()> {
    items.push(LocalItem {
        path: dir.to_owned(),
        relative: relative.clone(),
        is_dir: true,
        size: None,
    });
    let mut children: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| AppError::Platform(format!("cannot read {}: {e}", dir.display())))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    children.sort();
    for child in children {
        let Ok(link_metadata) = std::fs::symlink_metadata(&child) else {
            continue;
        };
        let name = component(&child)?;
        let mut child_relative = relative.clone();
        child_relative.push(name);
        if link_metadata.is_dir() {
            walk_dir(&child, child_relative, items)?;
        } else if let Ok(metadata) = std::fs::metadata(&child)
            && metadata.is_file()
        {
            items.push(LocalItem {
                path: child,
                relative: child_relative,
                is_dir: false,
                size: Some(metadata.len()),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitizes_untrusted_names() {
        for (raw, unix, windows) in [
            ("report.pdf", "report.pdf", "report.pdf"),
            ("..", "_", "_"),
            (".", "_", "_"),
            ("", "_", "_"),
            ("a/b", "a_b", "a_b"),
            ("a\\b", "a_b", "a_b"),
            ("line\nbreak\t", "line_break_", "line_break_"),
            ("what?.txt", "what?.txt", "what_.txt"),
            ("a:b*c|d\"e<f>", "a:b*c|d\"e<f>", "a_b_c_d_e_f_"),
            ("trailing. . ", "trailing. . ", "trailing"),
            ("CON", "CON", "_CON"),
            ("con.txt", "con.txt", "_con.txt"),
            ("lpt9.tar.gz", "lpt9.tar.gz", "_lpt9.tar.gz"),
            ("console.log", "console.log", "console.log"),
            ("...", "...", "_"),
        ] {
            assert_eq!(sanitize_name_for(raw, false), unix, "{raw:?} unix");
            assert_eq!(sanitize_name_for(raw, true), windows, "{raw:?} windows");
        }
        let long = "é".repeat(200);
        let cut = sanitize_name_for(&long, false);
        assert!(cut.len() <= 255 && cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn numbers_names_to_keep_both() {
        assert_eq!(numbered_name("report.pdf", 1), "report (1).pdf");
        assert_eq!(numbered_name("archive.tar.gz", 2), "archive.tar (2).gz");
        assert_eq!(numbered_name("Makefile", 3), "Makefile (3)");
        assert_eq!(numbered_name(".env", 1), ".env (1)");
    }

    #[test]
    fn part_files_sit_next_to_the_target() {
        assert_eq!(
            part_path(Path::new("/tmp/dl/a.txt")),
            PathBuf::from("/tmp/dl/a.txt.gem-part")
        );
    }

    #[test]
    fn walks_folders_parents_first() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("site");
        std::fs::create_dir_all(root.join("css")).unwrap();
        std::fs::write(root.join("index.html"), b"<p>").unwrap();
        std::fs::write(root.join("css/a.css"), b"a{}").unwrap();
        let single = dir.path().join("note.txt");
        std::fs::write(&single, b"hi").unwrap();
        let items = walk(&[root.clone(), single.clone()]).unwrap();
        let rel: Vec<String> = items.iter().map(|i| i.relative.join("/")).collect();
        assert_eq!(
            rel,
            [
                "site",
                "site/css",
                "site/css/a.css",
                "site/index.html",
                "note.txt"
            ]
        );
        assert!(items[0].is_dir && items[1].is_dir);
        assert_eq!(items[2].size, Some(3));
        #[cfg(unix)]
        {
            // A link to a folder inside the tree is skipped, so loops cannot happen.
            std::os::unix::fs::symlink(&root, root.join("loop")).unwrap();
            let again = walk(std::slice::from_ref(&root)).unwrap();
            assert!(
                again
                    .iter()
                    .all(|i| !i.relative.contains(&"loop".to_owned()))
            );
        }
    }
}
