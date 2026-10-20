//! Relative paths taken from untrusted input, such as the files in a unit's
//! `src/` tree.

use std::fmt;
use std::str::FromStr;

/// A relative file path that stays inside the directory it is joined to, on
/// every supported platform.
///
/// The path is a sequence of segments separated by `/`. It is validated as a
/// string, independently of the platform Assemblr runs on, so a unit is
/// accepted or rejected the same way everywhere. A path is rejected, never
/// normalized, when:
///
/// - it is empty, or starts with `/`, which makes it absolute (and `//` a UNC
///   path);
/// - it contains `\`, which Windows treats as a separator, or NUL;
/// - a segment is empty, `.`, or `..`;
/// - a segment contains `:`, which Windows reads as a drive prefix (`C:x`) or
///   an alternate data stream (`file:stream`);
/// - a segment ends with `.` or a space, which Windows strips, so that
///   `.. ` would name the parent directory and `a.` the file `a`;
/// - a segment is a name Windows reserves for devices, such as `CON`, `nul`,
///   `COM1`, or `aux.txt`, in any case and with any extension.
///
/// Paths order by their text.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RelativePath(String);

impl RelativePath {
    /// Parses a relative path, rejecting anything that could escape its root
    /// or name something other than a plain file on some platform.
    pub fn parse(input: &str) -> Result<Self, RelativePathError> {
        match check(input) {
            None => Ok(Self(input.to_owned())),
            Some(reason) => Err(RelativePathError {
                input: input.to_owned(),
                reason,
            }),
        }
    }

    /// The path as written, with `/` separators.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// The path's segments, in order.
    pub fn segments(&self) -> impl Iterator<Item = &str> {
        self.0.split('/')
    }
}

impl FromStr for RelativePath {
    type Err = RelativePathError;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

impl fmt::Display for RelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The first rule `input` breaks, if any.
fn check(input: &str) -> Option<RelativePathErrorReason> {
    if input.is_empty() {
        return Some(RelativePathErrorReason::Empty);
    }
    if input.contains('\0') {
        return Some(RelativePathErrorReason::Nul);
    }
    if input.contains('\\') {
        return Some(RelativePathErrorReason::Backslash);
    }
    if input.starts_with('/') {
        return Some(RelativePathErrorReason::Absolute);
    }
    input.split('/').find_map(check_segment)
}

fn check_segment(segment: &str) -> Option<RelativePathErrorReason> {
    let invalid = |reason: fn(String) -> RelativePathErrorReason| Some(reason(segment.to_owned()));
    match segment {
        "" => Some(RelativePathErrorReason::EmptySegment),
        "." => Some(RelativePathErrorReason::CurrentDirectory),
        ".." => Some(RelativePathErrorReason::ParentDirectory),
        _ if segment.contains(':') => invalid(RelativePathErrorReason::Colon),
        _ if segment.ends_with(['.', ' ']) => invalid(RelativePathErrorReason::TrailingDotOrSpace),
        _ if is_reserved_on_windows(segment) => invalid(RelativePathErrorReason::ReservedName),
        _ => None,
    }
}

/// Whether Windows reserves `segment` for a device. The reservation applies
/// to the text before the first `.`, ignoring case and trailing spaces, so
/// `nul.txt` and `Con .tar.gz` are reserved too.
fn is_reserved_on_windows(segment: &str) -> bool {
    let stem = segment
        .split('.')
        .next()
        .unwrap_or(segment)
        .trim_end_matches(' ');
    const DEVICES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    if DEVICES
        .iter()
        .any(|device| stem.eq_ignore_ascii_case(device))
    {
        return true;
    }
    // COM and LPT ports take a digit or a superscript one, two, or three.
    ["COM", "LPT"].iter().any(|port| {
        stem.get(..port.len())
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case(port))
            && matches!(
                &stem[port.len()..],
                "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
    })
}

/// A string that is not a valid relative path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelativePathError {
    input: String,
    reason: RelativePathErrorReason,
}

impl RelativePathError {
    /// The rejected input, exactly as given.
    pub fn input(&self) -> &str {
        &self.input
    }

    /// The first rule the input breaks.
    pub fn reason(&self) -> &RelativePathErrorReason {
        &self.reason
    }
}

impl fmt::Display for RelativePathError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "invalid relative path {:?}: ", self.input)?;
        match &self.reason {
            RelativePathErrorReason::Empty => f.write_str("it is empty"),
            RelativePathErrorReason::Absolute => {
                f.write_str("it starts with `/`; paths must be relative")
            }
            RelativePathErrorReason::Backslash => {
                f.write_str("it contains `\\`; use `/` to separate segments")
            }
            RelativePathErrorReason::Nul => f.write_str("it contains a NUL character"),
            RelativePathErrorReason::EmptySegment => {
                f.write_str("it has an empty segment, from a trailing or doubled `/`")
            }
            RelativePathErrorReason::CurrentDirectory => f.write_str("it has a `.` segment"),
            RelativePathErrorReason::ParentDirectory => {
                f.write_str("it has a `..` segment, which could leave its directory")
            }
            RelativePathErrorReason::Colon(segment) => write!(
                f,
                "segment {segment:?} contains `:`, which Windows reads as a drive or a stream"
            ),
            RelativePathErrorReason::TrailingDotOrSpace(segment) => write!(
                f,
                "segment {segment:?} ends with a dot or a space, which Windows removes"
            ),
            RelativePathErrorReason::ReservedName(segment) => write!(
                f,
                "segment {segment:?} is a device name that Windows reserves"
            ),
        }
    }
}

impl std::error::Error for RelativePathError {}

/// The rule an invalid relative path breaks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelativePathErrorReason {
    /// The path is empty.
    Empty,
    /// The path starts with `/`.
    Absolute,
    /// The path contains a backslash.
    Backslash,
    /// The path contains a NUL character.
    Nul,
    /// A segment is empty.
    EmptySegment,
    /// A segment is `.`.
    CurrentDirectory,
    /// A segment is `..`.
    ParentDirectory,
    /// This segment contains `:`.
    Colon(String),
    /// This segment ends with `.` or a space.
    TrailingDotOrSpace(String),
    /// This segment is a device name reserved on Windows.
    ReservedName(String),
}

#[cfg(test)]
mod tests {
    use super::{RelativePath, RelativePathErrorReason};

    fn reason(input: &str) -> RelativePathErrorReason {
        RelativePath::parse(input).unwrap_err().reason().clone()
    }

    #[test]
    fn ordinary_relative_paths_are_accepted_as_written() {
        for input in [
            "README.md",
            "src/main.rs",
            ".github/workflows/ci.yml",
            ".hidden",
            "foo..bar",
            "a/b.c/d",
            "con-x",
            "console.log",
            "COM10",
            "lpt",
            "auxiliary/nul-terminated.txt",
            "with space/file name.txt",
            "unicode/café.txt",
        ] {
            let path = RelativePath::parse(input).unwrap();
            assert_eq!(path.as_str(), input);
            assert_eq!(path.to_string(), input);
        }
    }

    #[test]
    fn segments_are_split_on_slashes() {
        let path = RelativePath::parse("a/b.c/d").unwrap();
        assert_eq!(path.segments().collect::<Vec<_>>(), ["a", "b.c", "d"]);
    }

    #[test]
    fn paths_that_could_leave_their_root_are_rejected() {
        use RelativePathErrorReason::{
            Absolute, Backslash, CurrentDirectory, EmptySegment, Nul, ParentDirectory,
        };
        for (input, expected) in [
            ("/etc/passwd", Absolute),
            ("//server/share/file", Absolute),
            ("..", ParentDirectory),
            ("../outside", ParentDirectory),
            ("a/../../outside", ParentDirectory),
            ("a/..", ParentDirectory),
            (".", CurrentDirectory),
            ("./a", CurrentDirectory),
            ("a/./b", CurrentDirectory),
            ("a//b", EmptySegment),
            ("a/", EmptySegment),
            ("a\\b", Backslash),
            ("..\\outside", Backslash),
            ("\\\\server\\share", Backslash),
            ("C:\\Windows", Backslash),
            ("a\0b", Nul),
        ] {
            assert_eq!(reason(input), expected, "{input:?}");
        }
        assert_eq!(reason(""), RelativePathErrorReason::Empty);
    }

    #[test]
    fn windows_drive_and_stream_syntax_is_rejected() {
        for (input, segment) in [
            ("C:", "C:"),
            ("C:/Windows", "C:"),
            ("c:file", "c:file"),
            ("a/file.txt:stream", "file.txt:stream"),
        ] {
            assert_eq!(
                reason(input),
                RelativePathErrorReason::Colon(segment.to_owned()),
                "{input:?}"
            );
        }
    }

    #[test]
    fn segments_windows_would_rewrite_are_rejected() {
        for (input, segment) in [
            ("a.", "a."),
            ("dir./file", "dir."),
            (".. /outside", ".. "),
            ("...", "..."),
            ("file ", "file "),
        ] {
            assert_eq!(
                reason(input),
                RelativePathErrorReason::TrailingDotOrSpace(segment.to_owned()),
                "{input:?}"
            );
        }
    }

    #[test]
    fn windows_device_names_are_rejected_in_any_segment() {
        for (input, segment) in [
            ("CON", "CON"),
            ("con", "con"),
            ("a/Nul.txt", "Nul.txt"),
            ("aux.tar.gz/file", "aux.tar.gz"),
            ("prn", "prn"),
            ("COM1", "COM1"),
            ("com0.log", "com0.log"),
            ("lpt9", "lpt9"),
            ("LPT¹", "LPT¹"),
            ("conin$", "conin$"),
            ("CONOUT$.txt", "CONOUT$.txt"),
            ("Con .txt", "Con .txt"),
        ] {
            assert_eq!(
                reason(input),
                RelativePathErrorReason::ReservedName(segment.to_owned()),
                "{input:?}"
            );
        }
    }

    #[test]
    fn the_first_problem_is_reported() {
        assert_eq!(reason("/a/../b"), RelativePathErrorReason::Absolute);
        assert_eq!(reason("a/../con"), RelativePathErrorReason::ParentDirectory);
        assert_eq!(
            reason("con/a:b"),
            RelativePathErrorReason::ReservedName("con".to_owned())
        );
    }

    #[test]
    fn errors_name_the_input_and_the_rule() {
        let error = RelativePath::parse("a/../b").unwrap_err();
        assert_eq!(error.input(), "a/../b");
        let message = error.to_string();
        assert!(message.contains("\"a/../b\""), "{message}");
        assert!(message.contains("`..`"), "{message}");

        let message = RelativePath::parse("docs/aux.md").unwrap_err().to_string();
        assert!(message.contains("\"aux.md\""), "{message}");
        assert!(message.contains("reserves"), "{message}");
    }
}
