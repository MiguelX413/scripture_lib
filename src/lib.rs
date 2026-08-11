#![warn(missing_docs)]

//! Read unpacked Digital Bible Library (DBL) bundles and select passages from
//! their USX scripture files.
//!
//! Start with [`ScriptureLibrary::discover`] to load a bundle or a directory of
//! bundles. Bundle metadata includes its localized display name, preferred
//! abbreviation, canonicalized [`icu_locale::Locale`], writing direction, and
//! available books. USX files are parsed on demand when [`Book::verses`] or
//! [`DblBundle::passage`] is called.
//! Call [`usx::book_code`] and [`usx::verses`] to parse a USX file without a
//! DBL bundle.
//!
//! Passage requests are structured values rather than strings. Constructors on
//! [`PassageRequest`] support one chapter, a chapter range, one verse, a verse
//! range within a chapter, or a range spanning chapters.
//!
//! # Example
//!
//! ```no_run
//! use scripture_lib::{PassageRequest, ScriptureLibrary};
//!
//! # fn main() -> Result<(), scripture_lib::Error> {
//! let library = ScriptureLibrary::discover("offline")?;
//! let bundle = library.get("LXXUP").expect("LXXUP bundle is installed");
//! let request = PassageRequest::verse_range("Genesis", 1, 2, 3, 4)?;
//! let passage = bundle.passage(&request)?;
//!
//! println!("{}", passage.text());
//! # Ok(())
//! # }
//! ```

mod bundle;
mod error;
mod metadata;
mod passage;
mod xml;

/// Direct parsing of Unified Scripture XML documents.
pub mod usx;

pub use bundle::{Book, BookNames, DblBundle, ScriptDirection, ScriptureLibrary};
pub use error::Error;
pub use passage::{
    InvalidPassageRequest, Passage, PassageRange, PassageRequest, Verse, VerseReference,
};
