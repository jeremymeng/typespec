//! Port of the subset of `packages/compiler/src/core/path-utils.ts` used by
//! multi-file source loading. Paths are kept in the TS compiler's normalized
//! form: forward slashes, `.`/`..` segments reduced, roots preserved.

const DIRECTORY_SEPARATOR: char = '/';
const URL_SCHEME_SEPARATOR: &str = "://";

/// Replace every `\` with `/`.
pub fn normalize_slashes(path: &str) -> String {
    path.replace('\\', "/")
}

fn is_any_directory_separator(ch: u8) -> bool {
    ch == b'/' || ch == b'\\'
}

fn is_volume_character(ch: u8) -> bool {
    ch.is_ascii_alphabetic()
}

fn file_url_volume_separator_end(url: &[u8], start: usize) -> Option<usize> {
    match url.get(start) {
        Some(b':') => Some(start + 1),
        Some(b'%') if url.get(start + 1) == Some(&b'3') => match url.get(start + 2) {
            Some(b'a') | Some(b'A') => Some(start + 3),
            _ => None,
        },
        _ => None,
    }
}

/// Mirrors TS `getEncodedRootLength`: a negative value is the ones-complement
/// root length of a URL.
fn encoded_root_length(path: &str) -> i64 {
    let bytes = path.as_bytes();
    let Some(&ch0) = bytes.first() else {
        return 0;
    };

    if ch0 == b'/' || ch0 == b'\\' {
        if bytes.get(1) != Some(&ch0) {
            return 1;
        }
        let separator = if ch0 == b'/' { '/' } else { '\\' };
        return match path[2..].find(separator) {
            None => path.len() as i64,
            Some(index) => (index + 2 + 1) as i64,
        };
    }

    if is_volume_character(ch0) && bytes.get(1) == Some(&b':') {
        match bytes.get(2) {
            Some(&ch2) if is_any_directory_separator(ch2) => return 3,
            None => return 2,
            _ => {}
        }
    }

    if let Some(scheme_end) = path.find(URL_SCHEME_SEPARATOR) {
        let authority_start = scheme_end + URL_SCHEME_SEPARATOR.len();
        if let Some(offset) = path[authority_start..].find(DIRECTORY_SEPARATOR) {
            let authority_end = authority_start + offset;
            let scheme = &path[..scheme_end];
            let authority = &path[authority_start..authority_end];
            if scheme == "file"
                && (authority.is_empty() || authority == "localhost")
                && bytes
                    .get(authority_end + 1)
                    .is_some_and(|ch| is_volume_character(*ch))
            {
                if let Some(volume_end) = file_url_volume_separator_end(bytes, authority_end + 2) {
                    if bytes.get(volume_end) == Some(&b'/') {
                        return !((volume_end + 1) as i64);
                    }
                    if volume_end == bytes.len() {
                        return !(volume_end as i64);
                    }
                }
            }
            return !((authority_end + 1) as i64);
        }
        return !(path.len() as i64);
    }

    0
}

/// Length of the root portion of a path (`/`, `c:/`, `//server/`, `file:///`).
pub fn root_length(path: &str) -> usize {
    let length = encoded_root_length(path);
    if length < 0 {
        (!length) as usize
    } else {
        length as usize
    }
}

pub fn is_path_absolute(path: &str) -> bool {
    encoded_root_length(path) != 0
}

fn has_trailing_directory_separator(path: &str) -> bool {
    path.as_bytes()
        .last()
        .is_some_and(|ch| is_any_directory_separator(*ch))
}

fn remove_trailing_directory_separator(path: &str) -> &str {
    if has_trailing_directory_separator(path) {
        &path[..path.len() - 1]
    } else {
        path
    }
}

fn ensure_trailing_directory_separator(path: &str) -> String {
    if has_trailing_directory_separator(path) {
        path.to_string()
    } else {
        format!("{path}{DIRECTORY_SEPARATOR}")
    }
}

/// Containing directory of `path` (TS `getDirectoryPath`).
pub fn get_directory_path(path: &str) -> String {
    let path = normalize_slashes(path);
    let root = root_length(&path);
    if root == path.len() {
        return path;
    }
    let trimmed = remove_trailing_directory_separator(&path);
    let end = trimmed
        .rfind(DIRECTORY_SEPARATOR)
        .map_or(root, |index| index.max(root));
    trimmed[..end.min(trimmed.len())].to_string()
}

/// Final path component (TS `getBaseFileName`).
pub fn get_base_file_name(path: &str) -> String {
    let path = normalize_slashes(path);
    let root = root_length(&path);
    if root == path.len() {
        return String::new();
    }
    let trimmed = remove_trailing_directory_separator(&path);
    let start = trimmed
        .rfind(DIRECTORY_SEPARATOR)
        .map_or(0, |index| index + 1)
        .max(root_length(trimmed));
    trimmed[start..].to_string()
}

/// Lower-cased extension including the dot, or an empty string.
pub fn get_any_extension_from_path(path: &str) -> String {
    let base = get_base_file_name(path);
    match base.rfind('.') {
        Some(index) => base[index..].to_lowercase(),
        None => String::new(),
    }
}

fn path_components(path: &str, root: usize) -> Vec<String> {
    let mut components = vec![path[..root].to_string()];
    let mut rest: Vec<String> = path[root..]
        .split(DIRECTORY_SEPARATOR)
        .map(str::to_string)
        .collect();
    if rest.last().is_some_and(String::is_empty) {
        rest.pop();
    }
    components.extend(rest);
    components
}

fn reduce_path_components(components: &[String]) -> Vec<String> {
    if components.is_empty() {
        return Vec::new();
    }
    let mut reduced = vec![components[0].clone()];
    for component in &components[1..] {
        if component.is_empty() || component == "." {
            continue;
        }
        if component == ".." {
            if reduced.len() > 1 {
                if reduced.last().is_some_and(|last| last != "..") {
                    reduced.pop();
                    continue;
                }
            } else if !reduced[0].is_empty() {
                continue;
            }
        }
        reduced.push(component.clone());
    }
    reduced
}

fn path_from_components(components: &[String]) -> String {
    let Some(first) = components.first() else {
        return String::new();
    };
    let root = if first.is_empty() {
        String::new()
    } else {
        ensure_trailing_directory_separator(first)
    };
    format!("{root}{}", components[1..].join("/"))
}

/// Combine paths without reducing `.`/`..` (TS `joinPaths`).
pub fn join_paths(path: &str, paths: &[&str]) -> String {
    let mut result = normalize_slashes(path);
    for relative in paths {
        if relative.is_empty() {
            continue;
        }
        let relative = normalize_slashes(relative);
        if result.is_empty() || root_length(&relative) != 0 {
            result = relative;
        } else {
            result = format!("{}{relative}", ensure_trailing_directory_separator(&result));
        }
    }
    result
}

/// Normalize slashes and reduce `.`/`..` segments (TS `normalizePath`).
pub fn normalize_path(path: &str) -> String {
    let path = normalize_slashes(path);
    let root = root_length(&path);
    let normalized = path_from_components(&reduce_path_components(&path_components(&path, root)));
    if !normalized.is_empty() && has_trailing_directory_separator(&path) {
        ensure_trailing_directory_separator(&normalized)
    } else {
        normalized
    }
}

/// Combine and normalize paths (TS `resolvePath`).
pub fn resolve_path(path: &str, paths: &[&str]) -> String {
    if paths.is_empty() {
        normalize_path(path)
    } else {
        normalize_path(&join_paths(path, paths))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn computes_root_lengths_like_ts() {
        assert_eq!(root_length("a"), 0);
        assert_eq!(root_length("/"), 1);
        assert_eq!(root_length("c:"), 2);
        assert_eq!(root_length("c:d"), 0);
        assert_eq!(root_length("c:/"), 3);
        assert_eq!(root_length("c:\\"), 3);
        assert_eq!(root_length("//server"), 8);
        assert_eq!(root_length("//server/share"), 9);
        assert_eq!(root_length("file:///path"), 8);
        assert_eq!(root_length("file:///c:/path"), 11);
        assert_eq!(root_length("http://server/path"), 14);
    }

    #[test]
    fn resolves_relative_segments() {
        assert_eq!(resolve_path("/a/b", &["./c.tsp"]), "/a/b/c.tsp");
        assert_eq!(resolve_path("/a/b", &["../c.tsp"]), "/a/c.tsp");
        assert_eq!(resolve_path("/a/b", &["/x/y.tsp"]), "/x/y.tsp");
        assert_eq!(
            resolve_path("Z:\\test", &["lib\\main.tsp"]),
            "Z:/test/lib/main.tsp"
        );
        assert_eq!(resolve_path("Z:/test", &["../../x.tsp"]), "Z:/x.tsp");
        assert_eq!(resolve_path("/a/b/", &[]), "/a/b/");
        assert_eq!(normalize_path("a/./b/../c"), "a/c");
        assert_eq!(normalize_path("../a"), "../a");
    }

    #[test]
    fn gets_directory_and_extension() {
        assert_eq!(get_directory_path("/a/b/c.tsp"), "/a/b");
        assert_eq!(get_directory_path("/a"), "/");
        assert_eq!(get_directory_path("/"), "/");
        assert_eq!(get_directory_path("Z:/test/main.tsp"), "Z:/test");
        assert_eq!(get_directory_path("Z:/main.tsp"), "Z:/");
        assert_eq!(get_any_extension_from_path("/a/b/c.TSP"), ".tsp");
        assert_eq!(get_any_extension_from_path("/a/b.d/c"), "");
        assert!(is_path_absolute("/a"));
        assert!(is_path_absolute("c:/a"));
        assert!(!is_path_absolute("./a"));
    }
}
