//! `file:` URIs, which is how a language server names a file.
//!
//! Two directions, and they have to agree on what counts as the same file: a
//! server is free to encode a path differently from how it was sent (`%3A` for
//! the colon of a Windows drive, a lowercased drive letter), so a URI coming
//! back is compared by [`key`], not as a string.

use std::path::{Path, PathBuf};

/// The URI for an absolute path.
pub fn from_path(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let mut out = String::from("file://");
    if !text.starts_with('/') {
        // `C:/x` is `file:///C:/x`.
        out.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// The path a `file:` URI names, if it is one.
pub fn to_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let decoded = decode(rest)?;
    // `/C:/x` on Windows is `C:/x`.
    let bytes = decoded.as_bytes();
    let trimmed = if bytes.len() > 2 && bytes[0] == b'/' && bytes[2] == b':' {
        &decoded[1..]
    } else {
        &decoded[..]
    };
    Some(PathBuf::from(trimmed))
}

/// What two URIs for the same file have in common.
pub fn key(uri: &str) -> String {
    let Some(path) = to_path(uri) else {
        return uri.to_string();
    };
    let text = path.to_string_lossy().replace('\\', "/");
    // A drive letter's case is the one difference servers disagree on.
    let mut chars: Vec<char> = text.chars().collect();
    if chars.len() > 1 && chars[1] == ':' {
        chars[0] = chars[0].to_ascii_lowercase();
    }
    chars.into_iter().collect()
}

fn decode(s: &str) -> Option<String> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = s.get(i + 1..i + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_unix_path_round_trips() {
        let path = Path::new("/home/me/my project/src/main.rs");
        let uri = from_path(path);
        assert_eq!(uri, "file:///home/me/my%20project/src/main.rs");
        assert_eq!(to_path(&uri).unwrap(), path);
    }

    #[test]
    fn a_windows_path_gets_a_third_slash() {
        assert_eq!(from_path(Path::new("C:/src/a.rs")), "file:///C:/src/a.rs");
        assert_eq!(
            to_path("file:///C:/src/a.rs").unwrap(),
            Path::new("C:/src/a.rs")
        );
    }

    #[test]
    fn differently_encoded_uris_for_one_file_share_a_key() {
        assert_eq!(key("file:///C:/src/a.rs"), key("file:///c%3A/src/a.rs"));
        assert_eq!(
            key("file:///x/a%20b.rs"),
            key(&from_path(Path::new("/x/a b.rs")))
        );
    }
}
