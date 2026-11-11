//! POSIX path helpers for remote paths (never `std::path`, which is OS-specific).

/// Joins a directory and a name.
pub fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() || dir == "/" {
        format!("/{}", name.trim_start_matches('/'))
    } else {
        format!(
            "{}/{}",
            dir.trim_end_matches('/'),
            name.trim_start_matches('/')
        )
    }
}

/// The parent directory; `/` for top-level entries and for `/` itself.
pub fn parent(path: &str) -> String {
    let trimmed = path.trim_end_matches('/');
    match trimmed.rfind('/') {
        Some(0) | None => "/".to_owned(),
        Some(index) => trimmed[..index].to_owned(),
    }
}

/// The last component.
pub fn file_name(path: &str) -> &str {
    let trimmed = path.trim_end_matches('/');
    trimmed.rsplit('/').next().unwrap_or(trimmed)
}

/// Collapses `//`, `.` and `..` lexically. Relative paths are resolved against `base`.
pub fn normalize(base: &str, path: &str) -> String {
    let full = if path.starts_with('/') {
        path.to_owned()
    } else {
        join(base, path)
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in full.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    format!("/{}", parts.join("/"))
}

/// Whether `name` is a single, safe path component (no separators, not `.`/`..`, no NUL).
pub fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.contains('/')
        && !name.contains('\0')
        && name.len() <= 255
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_and_splits() {
        assert_eq!(join("/", "a"), "/a");
        assert_eq!(join("/var/www/", "a"), "/var/www/a");
        assert_eq!(join("", "a"), "/a");
        assert_eq!(parent("/var/www/a"), "/var/www");
        assert_eq!(parent("/a"), "/");
        assert_eq!(parent("/"), "/");
        assert_eq!(file_name("/var/www/a.txt"), "a.txt");
        assert_eq!(file_name("/var/www/"), "www");
    }

    #[test]
    fn normalizes_lexically() {
        assert_eq!(normalize("/home/u", "../x/./y//z"), "/home/x/y/z");
        assert_eq!(normalize("/home/u", "/etc/../tmp"), "/tmp");
        assert_eq!(normalize("/", "../../.."), "/");
        assert_eq!(normalize("/a", ""), "/a");
    }

    #[test]
    fn validates_names() {
        assert!(is_valid_name("index.php"));
        assert!(is_valid_name(".env"));
        for bad in ["", ".", "..", "a/b", "a\0b"] {
            assert!(!is_valid_name(bad), "{bad:?}");
        }
        assert!(!is_valid_name(&"x".repeat(256)));
    }
}
