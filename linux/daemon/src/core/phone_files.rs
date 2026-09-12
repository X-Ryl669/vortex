//! One folder on the phone, as the laptop sees it.
//!
//! The phone answers a `browse` key in the bulk-sync request with a listing:
//! what is in that folder, not the folder itself. Nothing is transferred until
//! the user picks a file, and then it comes back over the ordinary file-pull
//! path — so this module is only the shape of the answer.
//!
//! The chunk envelope is the same `[total][idx][data]` every bulk dataset
//! uses, so the parser and the assembler are the contacts ones under names
//! that read correctly here rather than a second copy of the same code.

pub use crate::core::contacts::parse_chunk;
/// Reassembles a chunked listing. Same envelope as every other bulk dataset.
pub use crate::core::contacts::ContactsAssembler as ListingAssembler;

use serde::{Deserialize, Serialize};

/// One row in a folder: a file, or a folder to descend into.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PhoneFileEntry {
    /// The phone's document URI. Opaque to us: we echo it back to browse into
    /// it or to ask for its bytes, and the phone refuses any it did not grant.
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub mime: String,
    #[serde(default)]
    pub bytes: u64,
    /// Last modified, unix millis as the phone reports it; 0 when unknown.
    #[serde(default)]
    pub modified: u64,
    #[serde(default)]
    pub dir: bool,
}

/// The answer to one `browse`.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct PhoneListing {
    /// The folder this lists — echoed from the request, empty for the roots.
    ///
    /// Echoed rather than assumed: two browses can be in flight when someone
    /// clicks quickly, and a reply that overtakes another would otherwise be
    /// shown as the contents of the folder they are now looking at.
    #[serde(default)]
    pub at: String,
    #[serde(default)]
    pub entries: Vec<PhoneFileEntry>,
    /// Set when the folder held more than the phone was willing to send.
    #[serde(default)]
    pub truncated: bool,
    /// Why there is nothing to show — no folder granted yet, or unreadable.
    /// Shown to the user instead of an empty folder, which would be a lie.
    #[serde(default)]
    pub error: Option<String>,
}

impl PhoneListing {
    /// Parse a reassembled listing. `None` when it isn't one.
    pub fn parse(json: &[u8]) -> Option<Self> {
        serde_json::from_slice(json).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_listing_carries_the_folder_it_answers_for() {
        let json = br#"{"at":"content://x/tree/y/document/y",
            "entries":[
              {"id":"content://x/tree/y/document/y%2Fa.pdf","name":"a.pdf",
               "mime":"application/pdf","bytes":120,"modified":1789,"dir":false},
              {"id":"content://x/tree/y/document/y%2Fsub","name":"sub",
               "mime":"vnd.android.document/directory","bytes":0,"dir":true}
            ]}"#;
        let l = PhoneListing::parse(json).expect("parses");
        assert_eq!(l.at, "content://x/tree/y/document/y");
        assert_eq!(l.entries.len(), 2);
        assert_eq!(l.entries[0].name, "a.pdf");
        assert_eq!(l.entries[0].bytes, 120);
        assert!(!l.entries[0].dir);
        assert!(l.entries[1].dir);
        assert!(l.error.is_none());
    }

    /// The phone says why rather than returning an empty folder, and a build
    /// that sends fields this one does not know must still parse.
    #[test]
    fn a_refusal_and_an_unknown_field_both_survive() {
        let l = PhoneListing::parse(
            br#"{"at":"","entries":[],"error":"not a granted folder","future":42}"#,
        )
        .expect("parses");
        assert_eq!(l.error.as_deref(), Some("not a granted folder"));
        assert!(l.entries.is_empty());
        assert!(!PhoneListing::parse(b"nonsense").is_some());
    }
}
