use std::ops::Deref;
use std::path::Path;
use std::path::PathBuf;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::Error as SerdeError;

use super::ABSOLUTE_PATH_BASE;
use super::AbsolutePathBuf;

/// Absolute filesystem path that round-trips as a native path string.
///
/// Rollout JSON keeps the host-native path (not a `file:` URI) so existing
/// permission records stay compatible. Deserialization accepts that native
/// form **and** `file:` / `file://` URIs, both of which resolve to
/// [`AbsolutePathBuf`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PathUri(AbsolutePathBuf);

impl PathUri {
    /// Wraps an already-absolute path.
    pub fn from_absolute_path(path: AbsolutePathBuf) -> Self {
        Self(path)
    }

    /// Borrow the inner absolute path.
    pub fn as_absolute_path(&self) -> &AbsolutePathBuf {
        &self.0
    }

    /// Unwrap the inner absolute path.
    pub fn into_absolute_path(self) -> AbsolutePathBuf {
        self.0
    }
}

impl From<AbsolutePathBuf> for PathUri {
    fn from(path: AbsolutePathBuf) -> Self {
        Self(path)
    }
}

impl From<PathUri> for AbsolutePathBuf {
    fn from(path: PathUri) -> Self {
        path.0
    }
}

impl Deref for PathUri {
    type Target = AbsolutePathBuf;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AsRef<AbsolutePathBuf> for PathUri {
    fn as_ref(&self) -> &AbsolutePathBuf {
        &self.0
    }
}

impl AsRef<Path> for PathUri {
    fn as_ref(&self) -> &Path {
        self.0.as_path()
    }
}

impl PartialEq<AbsolutePathBuf> for PathUri {
    fn eq(&self, other: &AbsolutePathBuf) -> bool {
        self.0 == *other
    }
}

impl PartialEq<PathUri> for AbsolutePathBuf {
    fn eq(&self, other: &PathUri) -> bool {
        *self == other.0
    }
}

impl Serialize for PathUri {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for PathUri {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;
        let path = match parse_file_uri(&raw) {
            Some(path) => path,
            None => PathBuf::from(raw),
        };
        let absolute = ABSOLUTE_PATH_BASE.with(|cell| match cell.borrow().as_deref() {
            Some(base) => Ok(AbsolutePathBuf::resolve_path_against_base(path, base)),
            None if path.is_absolute() || path.has_root() => {
                AbsolutePathBuf::from_absolute_path(path).map_err(SerdeError::custom)
            }
            None => Err(SerdeError::custom(
                "PathUri deserialized without a base path",
            )),
        })?;
        Ok(Self(absolute))
    }
}

/// Parses `file:` / `file://` URIs into a filesystem path.
///
/// Returns `None` when `input` is a native path (or any non-file URI).
fn parse_file_uri(input: &str) -> Option<PathBuf> {
    let rest = input.strip_prefix("file:").or_else(|| {
        input
            .get(..5)
            .and_then(|prefix| prefix.eq_ignore_ascii_case("file:").then_some(&input[5..]))
    })?;
    let decoded = percent_decode(rest);

    let path = if let Some(after_host) = decoded.strip_prefix("//") {
        if after_host.starts_with('/') {
            after_host.to_string()
        } else if let Some(slash) = after_host.find('/') {
            let host = &after_host[..slash];
            let path = &after_host[slash..];
            if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
                path.to_string()
            } else if is_windows_drive_host(host) {
                format!("{host}{path}")
            } else {
                format!("//{after_host}")
            }
        } else {
            after_host.to_string()
        }
    } else {
        decoded
    };

    Some(native_path_from_file_uri_path(&path))
}

fn is_windows_drive_host(host: &str) -> bool {
    let bytes = host.as_bytes();
    bytes.len() == 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic()
}

fn native_path_from_file_uri_path(path: &str) -> PathBuf {
    #[cfg(windows)]
    {
        let mut path = path.to_string();
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0] == b'/'
            && bytes[1].is_ascii_alphabetic()
            && bytes[2] == b':'
        {
            path.remove(0);
        }
        PathBuf::from(path.replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        PathBuf::from(path)
    }
}

fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let Ok(value) = u8::from_str_radix(
                std::str::from_utf8(&bytes[index + 1..index + 3]).unwrap_or(""),
                16,
            )
        {
            out.push(value);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::PathUri;
    use super::parse_file_uri;
    use crate::absolute_path::AbsolutePathBuf;
    use pretty_assertions::assert_eq;
    use std::path::Path;

    #[cfg(windows)]
    fn windows_abs(path: &str) -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path(path).expect("absolute windows path")
    }

    #[cfg(unix)]
    fn unix_abs(path: &str) -> AbsolutePathBuf {
        AbsolutePathBuf::from_absolute_path(path).expect("absolute unix path")
    }

    #[cfg(windows)]
    #[test]
    fn serializes_as_native_path_string() {
        let uri = PathUri::from(windows_abs(r"C:\workspace\readme.md"));
        let json = serde_json::to_string(&uri).expect("serialize");
        assert_eq!(
            json,
            serde_json::to_string(r"C:\workspace\readme.md").unwrap()
        );
    }

    #[cfg(windows)]
    #[test]
    fn deserializes_native_path_and_file_uris() {
        let expected = windows_abs(r"C:\workspace\readme.md");
        let native: PathUri = serde_json::from_str(r#""C:\\workspace\\readme.md""#)
            .expect("native path should deserialize");
        assert_eq!(native.as_absolute_path(), &expected);

        let file_uri: PathUri = serde_json::from_str(r#""file:///C:/workspace/readme.md""#)
            .expect("file URI should deserialize");
        assert_eq!(file_uri, expected);

        let short_uri: PathUri = serde_json::from_str(r#""file:/C:/workspace/readme.md""#)
            .expect("file: URI should deserialize");
        assert_eq!(short_uri.as_path(), Path::new(r"C:\workspace\readme.md"));
    }

    #[cfg(unix)]
    #[test]
    fn serializes_as_native_path_string() {
        let uri = PathUri::from(unix_abs("/tmp/workspace/readme.md"));
        let json = serde_json::to_string(&uri).expect("serialize");
        assert_eq!(json, "\"/tmp/workspace/readme.md\"");
    }

    #[cfg(unix)]
    #[test]
    fn deserializes_native_path_and_file_uris() {
        let expected = unix_abs("/tmp/workspace/readme.md");
        let native: PathUri =
            serde_json::from_str("\"/tmp/workspace/readme.md\"").expect("native path");
        assert_eq!(native.as_absolute_path(), &expected);

        let file_uri: PathUri = serde_json::from_str("\"file:///tmp/workspace/readme.md\"")
            .expect("file URI should deserialize");
        assert_eq!(file_uri, expected);

        let short_uri: PathUri = serde_json::from_str("\"file:/tmp/workspace/readme.md\"")
            .expect("file: URI should deserialize");
        assert_eq!(short_uri.as_path(), Path::new("/tmp/workspace/readme.md"));
    }

    #[cfg(windows)]
    #[test]
    fn parse_file_uri_accepts_localhost_and_drive_host() {
        assert_eq!(
            parse_file_uri(r"file://localhost/C:/workspace/a.txt").as_deref(),
            Some(Path::new(r"C:\workspace\a.txt"))
        );
        assert_eq!(
            parse_file_uri(r"file://C:/workspace/a.txt").as_deref(),
            Some(Path::new(r"C:\workspace\a.txt"))
        );
        assert_eq!(parse_file_uri(r"C:\workspace\a.txt"), None);
    }

    #[cfg(unix)]
    #[test]
    fn parse_file_uri_leaves_native_paths_alone() {
        assert_eq!(parse_file_uri("/tmp/a.txt"), None);
        assert_eq!(
            parse_file_uri("file:///tmp/a%20b.txt").as_deref(),
            Some(Path::new("/tmp/a b.txt"))
        );
    }
}
