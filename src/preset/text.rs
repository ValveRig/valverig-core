//! The character rules every string in a document obeys.
//!
//! A `.rig` document is read by an engine, edited by hand, and shown in a
//! user interface, so its strings answer to all three. Two shapes carry the
//! weight:
//!
//! * an **identifier** (a parameter id, an asset label, a module type part,
//!   a presentation hint key) is machine vocabulary, so it is restricted to
//!   one spelling: lowercase ASCII, digits, `_` and `-`. One concept, one
//!   spelling, no case-folding anywhere.
//! * a **name**, which is what a module or lane is called, is a label a
//!   person chooses and the engine addresses. It may be any text, including
//!   punctuation and non-ASCII, because `A.capture` and `Ampli·Té` are both
//!   reasonable; it may not contain `/`, which separates a name from a
//!   parameter id in an address, and it may not contain control characters,
//!   which have no place in a label.
//!
//! Every rule is bounded. A document is untrusted input, and an unbounded
//! string is an unbounded allocation in whoever displays it.

/// The longest a module or lane name may be, in bytes.
pub(crate) const NAME_MAX: usize = 128;

/// The longest an identifier may be, in bytes.
pub(crate) const IDENTIFIER_MAX: usize = 64;

/// The longest a `file` hint may be, in bytes. The shortest limit a common
/// filesystem imposes on one path component.
pub(crate) const FILE_NAME_MAX: usize = 255;

/// The identifier rule, worded for an error message.
pub(crate) const IDENTIFIER_RULE: &str =
    "1 to 64 bytes of lowercase ASCII letters, digits, `_` or `-`";

/// The file-name rule, worded for an error message.
pub(crate) const FILE_NAME_RULE: &str =
    "1 to 255 bytes, no `/` or `\\`, not `.` or `..`, and no control characters";

/// Is this machine vocabulary: a parameter id, an asset label, a module type
/// part, or a presentation hint key?
pub(crate) fn is_identifier(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= IDENTIFIER_MAX
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-')
}

/// Is this something a module or lane may be called, and half of an address?
pub(crate) fn is_name(s: &str) -> bool {
    !s.is_empty() && s.len() <= NAME_MAX && !s.contains('/') && !s.chars().any(char::is_control)
}

/// Is this a namespaced module type, `std.gate`?
pub(crate) fn is_type_id(s: &str) -> bool {
    matches!(s.split_once('.'), Some((namespace, name)) if is_identifier(namespace) && is_identifier(name))
}

/// Is this a plain file name: one component, no directory part?
///
/// A hint that could name a path would let a document choose which file every
/// host opens. Rejecting it here means a resolver that joins the hint to its
/// own directory cannot be walked out of that directory, and one that trusts
/// the hint whole cannot be pointed at an absolute path.
pub(crate) fn is_file_name(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= FILE_NAME_MAX
        && s != "."
        && s != ".."
        && !s.contains(['/', '\\'])
        && !s.chars().any(char::is_control)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_have_one_spelling() {
        for ok in [
            "a",
            "level_db",
            "std",
            "a-b",
            "x9",
            &"n".repeat(IDENTIFIER_MAX),
        ] {
            assert!(is_identifier(ok), "{ok:?} should be an identifier");
        }
        for bad in [
            "",
            "A",
            "Level_db",
            "a.b",
            "a/b",
            "a b",
            "é",
            &"n".repeat(IDENTIFIER_MAX + 1),
        ] {
            assert!(!is_identifier(bad), "{bad:?} should not be an identifier");
        }
    }

    #[test]
    fn names_are_labels_that_can_be_addressed() {
        for ok in [
            "A",
            "A.capture",
            "Ampli·Té",
            "gate 1",
            &"n".repeat(NAME_MAX),
        ] {
            assert!(is_name(ok), "{ok:?} should be a name");
        }
        for bad in [
            "",
            "in/out",
            "a\nb",
            "a\tb",
            "a\0b",
            &"n".repeat(NAME_MAX + 1),
        ] {
            assert!(!is_name(bad), "{bad:?} should not be a name");
        }
    }

    #[test]
    fn type_ids_are_two_identifiers() {
        for ok in ["std.gate", "std.a-b", "x.y"] {
            assert!(is_type_id(ok), "{ok:?} should be a type id");
        }
        for bad in [
            "gate",
            "Std.Gate",
            "std.",
            ".gate",
            "std.gate.extra",
            "std gate",
            "",
        ] {
            assert!(!is_type_id(bad), "{bad:?} should not be a type id");
        }
    }

    #[test]
    fn a_file_hint_cannot_name_a_path() {
        for ok in [
            "a2.nam",
            "brit-plexi-50-v2.nam",
            "Cab 4x12.wav",
            &"n".repeat(FILE_NAME_MAX),
        ] {
            assert!(is_file_name(ok), "{ok:?} should be a file name");
        }
        for bad in [
            "",
            ".",
            "..",
            "sub/dir/a.nam",
            "../../../etc/passwd",
            "/etc/passwd",
            "a\\b.nam",
            "C:\\Windows",
            "a\0b",
            &"n".repeat(FILE_NAME_MAX + 1),
        ] {
            assert!(!is_file_name(bad), "{bad:?} should not be a file name");
        }
    }
}
