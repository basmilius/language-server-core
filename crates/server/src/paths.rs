//! `file:` URIs and the paths they name.

use std::path::{Path, PathBuf};
use std::str::FromStr;

use lsp_types::Uri;

/// The path a `file:` URI names, or `None` for another scheme or an encoding that is no UTF-8.
pub fn uri_to_path(uri: &Uri) -> Option<PathBuf> {
    let text = uri.as_str();
    let rest = text.strip_prefix("file://")?;
    // `file://host/path` has a host; the empty host is the usual case.
    let path = rest.find('/').map_or("", |index| &rest[index..]);
    let decoded = percent_decode(path)?;

    Some(PathBuf::from(native_path(decoded)))
}

/// What a decoded URI path is on this platform: on Windows `/C:/x` is `C:\x`.
fn native_path(decoded: String) -> String {
    if !cfg!(windows) {
        return decoded;
    }

    let bytes = decoded.as_bytes();
    let drive = bytes.len() >= 3 && bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':';
    let path = if drive { &decoded[1..] } else { &decoded[..] };

    path.replace('/', "\\")
}

/// The `file:` URI of an absolute path, with every byte outside the unreserved characters, `/`
/// and the sub-delimiters percent-encoded. `None` for a path that is no UTF-8.
pub fn path_to_uri(path: &Path) -> Option<Uri> {
    let text = uri_path(path.to_str()?);
    let mut out = String::from("file://");

    for (index, byte) in text.bytes().enumerate() {
        match byte {
            // The colon of a drive stays as it is: `/C:/x`.
            b':' if cfg!(windows) && index == 2 => out.push(':'),
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'.'
            | b'_'
            | b'~'
            | b'/'
            | b'$'
            | b'&'
            | b'+'
            | b','
            | b';'
            | b'='
            | b'@'
            | b'!'
            | b'('
            | b')'
            | b'\''
            | b'*' => {
                out.push(byte as char);
            }
            other => out.push_str(&format!("%{other:02X}")),
        }
    }

    Uri::from_str(&out).ok()
}

/// The path part of a URI: on Windows `C:\x` is `/C:/x`.
fn uri_path(path: &str) -> String {
    if !cfg!(windows) {
        return path.to_string();
    }

    let slashed = path.replace('\\', "/");

    if slashed.starts_with('/') {
        slashed
    } else {
        format!("/{slashed}")
    }
}

fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;

    while index < bytes.len() {
        if bytes[index] == b'%' {
            let hex = text.get(index + 1..index + 3)?;
            out.push(u8::from_str_radix(hex, 16).ok()?);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }

    String::from_utf8(out).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn uri(text: &str) -> Uri {
        Uri::from_str(text).expect("a uri")
    }

    #[test]
    fn converts_between_uris_and_paths() {
        let uri = uri("file:///Users/me/My%20Project/a%C3%A9.php");
        let path = uri_to_path(&uri).expect("a path");
        assert_eq!(path, PathBuf::from("/Users/me/My Project/aé.php"));
        assert_eq!(path_to_uri(&path).expect("a uri"), uri);
    }

    #[test]
    fn round_trips_paths_with_characters_that_need_encoding() {
        let paths = [
            "/plain/file.sql",
            "/with space/and%percent",
            "/hash#and?question",
            "/unicode/\u{e9}\u{4e2d}\u{1f600}.txt",
            "/brackets/[x]{y}<z>",
            "/kept/$&+,;=@!()'*~-._",
            "/back\\slash/and\"quote",
            "/",
        ];

        for text in paths {
            let path = PathBuf::from(text);
            let uri = path_to_uri(&path).expect("a uri");
            assert_eq!(uri_to_path(&uri).expect("a path"), path, "{}", uri.as_str());
        }
    }

    #[test]
    fn encodes_only_what_has_to_be_encoded() {
        let encoded = path_to_uri(Path::new("/a b/\u{e9}#?%/$&+,;=@!()'*~-._")).expect("a uri");
        assert_eq!(encoded.as_str(), "file:///a%20b/%C3%A9%23%3F%25/$&+,;=@!()'*~-._");
    }

    #[test]
    fn decodes_lowercase_and_uppercase_escapes() {
        assert_eq!(
            uri_to_path(&uri("file:///a%2fb%2Fc")).expect("a path"),
            PathBuf::from("/a/b/c")
        );
    }

    #[test]
    fn reads_a_uri_with_a_host_as_the_path_after_it() {
        assert_eq!(
            uri_to_path(&uri("file://server/share/x.php")).expect("a path"),
            PathBuf::from("/share/x.php")
        );
        assert_eq!(uri_to_path(&uri("file://server")).expect("a path"), PathBuf::from(""));
    }

    #[test]
    #[cfg(not(windows))]
    fn a_windows_drive_stays_after_the_slash() {
        assert_eq!(
            uri_to_path(&uri("file:///C:/Users/x.php")).expect("a path"),
            PathBuf::from("/C:/Users/x.php")
        );
    }

    #[test]
    #[cfg(windows)]
    fn a_windows_path_is_a_drive_and_back() {
        let path = uri_to_path(&uri("file:///C:/Users/My%20Project/x.php")).expect("a path");

        assert_eq!(path.to_str(), Some(r"C:\Users\My Project\x.php"));
        assert_eq!(
            path_to_uri(&path).expect("a uri").as_str(),
            "file:///C:/Users/My%20Project/x.php"
        );
        assert_eq!(
            uri_to_path(&uri("file:///c%3A/x.php")).expect("a path").to_str(),
            Some(r"c:\x.php")
        );
    }

    #[test]
    fn refuses_other_schemes_and_broken_escapes() {
        assert_eq!(uri_to_path(&uri("untitled:Untitled-1")), None);
        assert_eq!(uri_to_path(&uri("https://example.com/x")), None);
        assert_eq!(uri_to_path(&uri("file:///a%FF")), None);
        assert_eq!(percent_decode("/a%2"), None);
        assert_eq!(percent_decode("/a%zz"), None);
        assert_eq!(percent_decode("/a%\u{e9}"), None);
    }
}
