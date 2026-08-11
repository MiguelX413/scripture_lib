use std::io;
use std::path::PathBuf;

use thiserror::Error;

use crate::passage::{InvalidPassageRequest, PassageRequest};

/// An error encountered while loading DBL metadata, parsing USX, or selecting a passage.
#[derive(Debug, Error)]
pub enum Error {
    /// A filesystem operation failed.
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    /// An XML document was malformed.
    #[error("could not parse XML: {0}")]
    Xml(#[from] quick_xml::Error),
    /// XML text could not be decoded with the document's encoding.
    #[error("could not decode XML text: {0}")]
    XmlEncoding(#[from] quick_xml::encoding::EncodingError),
    /// An XML character or entity reference was invalid.
    #[error("could not unescape XML text: {0}")]
    XmlEscape(#[from] quick_xml::escape::EscapeError),
    /// A passage request violates its construction invariants.
    #[error(transparent)]
    InvalidPassageRequest(#[from] InvalidPassageRequest),
    /// The requested directory does not contain `metadata.xml`.
    #[error("DBL bundle at {0} has no metadata.xml")]
    MissingMetadata(PathBuf),
    /// A required DBL metadata field is absent.
    #[error("DBL metadata is missing {0}")]
    MissingField(&'static str),
    /// A DBL language identifier is not a valid locale.
    #[error("invalid locale {value:?}: {reason}")]
    InvalidLocale {
        /// Locale identifier read from DBL metadata.
        value: String,
        /// Diagnostic reported by the locale parser.
        reason: String,
    },
    /// A required USX element or attribute is absent.
    #[error("USX document is missing required {0}")]
    MissingUsxField(&'static str),
    /// The document declares a USX version unsupported by this crate.
    #[error("unsupported USX version {0:?}")]
    UnsupportedUsxVersion(String),
    /// The USX book code differs from the code declared by DBL metadata.
    #[error("USX book code {actual:?} does not match expected code {expected:?}")]
    MismatchedBookCode {
        /// Book code declared by DBL metadata.
        expected: String,
        /// Book code found in the USX document.
        actual: String,
    },
    /// A USX chapter end refers to a different chapter than the open chapter.
    #[error("USX chapter end {actual:?} does not match open chapter {expected:?}")]
    MismatchedChapterEnd {
        /// Scripture identifier of the open chapter.
        expected: String,
        /// Scripture identifier on the chapter-ending milestone.
        actual: String,
    },
    /// A chapter-ending milestone appears when no chapter is open.
    #[error("USX chapter end {0:?} has no corresponding open chapter")]
    UnexpectedChapterEnd(String),
    /// A USX scripture identifier does not describe its milestone.
    #[error("invalid {attribute} {actual:?}; expected {expected:?}")]
    InvalidUsxIdentifier {
        /// Attribute containing the invalid identifier.
        attribute: &'static str,
        /// Identifier implied by the milestone's book and number attributes.
        expected: String,
        /// Identifier read from the document.
        actual: String,
    },
    /// A verse-ending milestone refers to a different verse than the open verse.
    #[error("USX verse end {actual:?} does not match open verse {expected:?}")]
    MismatchedVerseEnd {
        /// Scripture identifier of the open verse.
        expected: String,
        /// Scripture identifier on the verse-ending milestone.
        actual: String,
    },
    /// A verse-ending milestone appears when no verse is open.
    #[error("USX verse end {0:?} has no corresponding open verse")]
    UnexpectedVerseEnd(String),
    /// A verse continuation refers to a different verse than the open verse.
    #[error("USX vid {actual:?} does not match open verse {expected:?}")]
    MismatchedVerseContinuation {
        /// Scripture identifier of the open verse.
        expected: String,
        /// Scripture identifier on the continuation element.
        actual: String,
    },
    /// A USX milestone that must be empty contains nested content.
    #[error("USX {0} milestones must be empty elements")]
    NonEmptyUsxMilestone(&'static str),
    /// A USX element uses a style that is invalid for its role.
    #[error("invalid style {actual:?} on USX {element}; expected {expected:?}")]
    InvalidUsxStyle {
        /// USX element containing the invalid style.
        element: &'static str,
        /// Style required by the USX specification in this context.
        expected: &'static str,
        /// Style read from the document.
        actual: String,
    },
    /// No book in the bundle matches the requested name or code.
    #[error("book {0:?} was not found in this bundle")]
    BookNotFound(String),
    /// The requested passage selects no verses in the bundle.
    #[error("passage {0:?} contains no verses in this bundle")]
    PassageNotFound(PassageRequest),
    /// A USX chapter number is not a positive integer.
    #[error("invalid USX chapter number {0:?}")]
    InvalidChapterNumber(String),
}
