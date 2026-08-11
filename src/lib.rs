#![warn(missing_docs)]

//! Read unpacked Digital Bible Library (DBL) bundles and select passages from
//! their USX scripture files.
//!
//! Start with [`ScriptureLibrary::discover`] to load a bundle or a directory of
//! bundles. Bundle metadata includes its localized display name, preferred
//! abbreviation, canonicalized [`icu_locale::Locale`], writing direction, and
//! available books. USX files are parsed on demand when [`Book::verses`] or
//! [`DblBundle::passage`] is called.
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
//! let request = PassageRequest::verse_range("Genesis", 1, 2, 3, 4);
//! let passage = bundle.passage(&request)?;
//!
//! println!("{}", passage.text());
//! # Ok(())
//! # }
//! ```

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufReader};
use std::path::{Path, PathBuf};

use icu_locale::{Locale, LocaleCanonicalizer};
use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};
use thiserror::Error;

const METADATA_FILE: &str = "metadata.xml";

/// A collection of unpacked DBL bundles found below one directory.
#[derive(Debug)]
pub struct ScriptureLibrary {
    bundles: Vec<DblBundle>,
}

impl ScriptureLibrary {
    /// Finds DBL bundles in `root`.
    ///
    /// If `root` is itself a bundle, it is returned as the only entry. Otherwise,
    /// each immediate child containing a `metadata.xml` file is loaded.
    pub fn discover(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = root.as_ref();
        if root.join(METADATA_FILE).is_file() {
            return Ok(Self {
                bundles: vec![DblBundle::open(root)?],
            });
        }

        let mut paths = fs::read_dir(root)?
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.is_dir() && path.join(METADATA_FILE).is_file())
            .collect::<Vec<_>>();
        paths.sort();

        let bundles = paths
            .into_iter()
            .map(DblBundle::open)
            .collect::<Result<_, _>>()?;
        Ok(Self { bundles })
    }

    /// Returns the discovered bundles in path order.
    pub fn bundles(&self) -> &[DblBundle] {
        &self.bundles
    }

    /// Finds a bundle by its preferred or non-localized abbreviation.
    ///
    /// Matching is ASCII case-insensitive. If multiple bundles use the same
    /// abbreviation, the first bundle in [`Self::bundles`] is returned.
    pub fn get(&self, abbreviation: &str) -> Option<&DblBundle> {
        self.bundles.iter().find(|bundle| {
            bundle.abbreviation.eq_ignore_ascii_case(abbreviation)
                || bundle
                    .metadata_abbreviation
                    .eq_ignore_ascii_case(abbreviation)
        })
    }
}

/// Metadata and available books for one unpacked DBL bundle.
#[derive(Debug)]
pub struct DblBundle {
    root: PathBuf,
    /// Unique DBL bundle identifier from `DBLMetadata/@id`.
    pub id: String,
    /// Optional DBL metadata revision number.
    pub revision: Option<u32>,
    /// Non-localized bundle name.
    pub name: String,
    /// Localized bundle name, when supplied by the metadata.
    pub local_name: Option<String>,
    /// Preferred display abbreviation (`abbreviationLocal` when available).
    pub abbreviation: String,
    /// The non-localized `abbreviation` value from DBL metadata.
    pub metadata_abbreviation: String,
    /// Localized `abbreviationLocal` value from DBL metadata, when present.
    pub local_abbreviation: Option<String>,
    /// Canonical scope declaration from DBL metadata, when present.
    pub scope: Option<String>,
    /// Canonicalized locale parsed from the metadata's LDML or ISO identifier.
    pub locale: Locale,
    /// Writing direction declared for the bundle's language.
    pub script_direction: ScriptDirection,
    books: Vec<Book>,
}

impl DblBundle {
    /// Opens an unpacked DBL bundle rooted at `root`.
    ///
    /// Metadata is read immediately, but book USX files are parsed only when
    /// their contents are requested.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, Error> {
        let root = root.as_ref();
        let metadata_path = root.join(METADATA_FILE);
        if !metadata_path.is_file() {
            return Err(Error::MissingMetadata(root.to_path_buf()));
        }

        let metadata = parse_metadata(&metadata_path)?;
        let locale_source = metadata
            .ldml
            .as_deref()
            .or(metadata.language_iso.as_deref())
            .ok_or(Error::MissingField("language/ldml or language/iso"))?;
        let mut locale = locale_source
            .parse::<Locale>()
            .map_err(|error| Error::InvalidLocale {
                value: locale_source.to_owned(),
                reason: error.to_string(),
            })?;
        LocaleCanonicalizer::new_extended().canonicalize(&mut locale);

        let metadata_abbreviation =
            required(metadata.abbreviation.clone(), "identification/abbreviation")?;
        let abbreviation = metadata
            .local_abbreviation
            .clone()
            .unwrap_or_else(|| metadata_abbreviation.clone());

        let books = metadata
            .usx_resources
            .into_iter()
            .map(|uri| {
                let path = root.join(&uri);
                let code = parse_usx_book_code(&path)?;
                let names = metadata.book_names.get(&code).cloned().unwrap_or_default();
                Ok(Book { code, names, path })
            })
            .collect::<Result<Vec<_>, Error>>()?;

        Ok(Self {
            root: root.to_path_buf(),
            id: required(metadata.id, "DBLMetadata/@id")?,
            revision: metadata.revision,
            name: required(metadata.name, "identification/name")?,
            local_name: metadata.local_name,
            abbreviation,
            metadata_abbreviation,
            local_abbreviation: metadata.local_abbreviation,
            scope: metadata.scope,
            locale,
            script_direction: metadata.script_direction.unwrap_or_default(),
            books,
        })
    }

    /// Returns the bundle's root directory.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Returns the books declared by the bundle's USX publication resources.
    pub fn books(&self) -> &[Book] {
        &self.books
    }

    /// Finds a book by its three-character USX code.
    ///
    /// Matching is ASCII case-insensitive.
    pub fn book(&self, code: &str) -> Option<&Book> {
        self.books
            .iter()
            .find(|book| book.code.eq_ignore_ascii_case(code))
    }

    /// Resolves a structured passage request against this bundle.
    ///
    /// The request's book can be a USX code or any book abbreviation, short
    /// name, or long name declared by this bundle.
    pub fn passage(&self, request: &PassageRequest) -> Result<Passage, Error> {
        let book = self
            .books
            .iter()
            .find(|book| book.matches_name(&request.book))
            .ok_or_else(|| Error::BookNotFound(request.book.clone()))?;
        book.request_passage(request)
    }
}

/// Display names declared for a book in DBL metadata.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BookNames {
    /// Short abbreviation, such as `Gen`.
    pub abbreviation: Option<String>,
    /// Short display name, such as `Genesis`.
    pub short: Option<String>,
    /// Long display name, when the bundle supplies one.
    pub long: Option<String>,
}

/// One USX book declared by a DBL bundle.
#[derive(Debug)]
pub struct Book {
    /// Three-character USX book code, such as `GEN`.
    pub code: String,
    /// Names and abbreviation declared for the book.
    pub names: BookNames,
    path: PathBuf,
}

impl Book {
    /// Returns the path to the book's USX file.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Reads the original USX document for this book.
    pub fn read_usx(&self) -> Result<String, Error> {
        Ok(fs::read_to_string(&self.path)?)
    }

    /// Parses the book's verses in document order.
    ///
    /// USX 3 `eid` milestones are honored when present. A following verse,
    /// chapter boundary, or end of file also closes the current verse, which
    /// supports documents with omitted verse-ending milestones.
    pub fn verses(&self) -> Result<Vec<Verse>, Error> {
        parse_verses(&self.path, &self.code)
    }

    fn matches_name(&self, name: &str) -> bool {
        self.code.eq_ignore_ascii_case(name)
            || [
                self.names.abbreviation.as_deref(),
                self.names.short.as_deref(),
                self.names.long.as_deref(),
            ]
            .into_iter()
            .flatten()
            .any(|candidate| candidate.eq_ignore_ascii_case(name))
    }

    fn request_passage(&self, request: &PassageRequest) -> Result<Passage, Error> {
        if !request.is_valid() {
            return Err(Error::InvalidPassageRequest {
                request: request.clone(),
                reason: "locations must be nonzero, consistently scoped, and ordered",
            });
        }

        let verses = self
            .verses()?
            .into_iter()
            .filter(|verse| request.includes(verse))
            .collect::<Vec<_>>();
        if verses.is_empty() {
            return Err(Error::PassageNotFound(request.clone()));
        }

        Ok(Passage {
            request: request.clone(),
            verses,
        })
    }
}

/// Plain scripture text associated with one USX verse milestone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verse {
    /// USX scripture reference, such as `GEN 1:1`.
    pub sid: String,
    /// Chapter number containing this verse.
    pub chapter: u32,
    /// Sequential verse number from USX `number`, including bridges such as `1-2`.
    pub number: String,
    /// Alternate-versification number from USX `altnumber`.
    pub alternate_number: Option<String>,
    /// Publication-facing number from USX `pubnumber`.
    pub published_number: Option<String>,
    /// Plain scripture text collected for the verse.
    pub text: String,
}

/// One endpoint in a passage request. `verse: None` means the whole chapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PassagePoint {
    /// One-based chapter number.
    pub chapter: u32,
    /// One-based verse number, or `None` when the endpoint is a whole chapter.
    pub verse: Option<u32>,
}

/// A parsed passage request, including its book name or USX code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PassageRequest {
    /// USX book code or a book name or abbreviation declared by the bundle.
    pub book: String,
    /// Inclusive start of the requested passage.
    pub start: PassagePoint,
    /// Inclusive end of the requested passage.
    pub end: PassagePoint,
}

impl PassageRequest {
    /// Requests one whole chapter, such as `Genesis 1`.
    pub fn chapter(book: impl Into<String>, chapter: u32) -> Self {
        Self::chapters(book, chapter, chapter)
    }

    /// Requests an inclusive range of whole chapters, such as `Genesis 1-2`.
    pub fn chapters(book: impl Into<String>, start_chapter: u32, end_chapter: u32) -> Self {
        Self {
            book: book.into(),
            start: PassagePoint {
                chapter: start_chapter,
                verse: None,
            },
            end: PassagePoint {
                chapter: end_chapter,
                verse: None,
            },
        }
    }

    /// Requests one verse, such as `Genesis 1:4`.
    pub fn verse(book: impl Into<String>, chapter: u32, verse: u32) -> Self {
        Self::verse_range(book, chapter, verse, chapter, verse)
    }

    /// Requests an inclusive verse range in one chapter, such as `Genesis 1:4-5`.
    pub fn verses(book: impl Into<String>, chapter: u32, start_verse: u32, end_verse: u32) -> Self {
        Self::verse_range(book, chapter, start_verse, chapter, end_verse)
    }

    /// Requests an inclusive verse range that may span chapters.
    ///
    /// For example, `verse_range("Genesis", 1, 4, 2, 3)` represents
    /// `Genesis 1:4-2:3`.
    pub fn verse_range(
        book: impl Into<String>,
        start_chapter: u32,
        start_verse: u32,
        end_chapter: u32,
        end_verse: u32,
    ) -> Self {
        Self {
            book: book.into(),
            start: PassagePoint {
                chapter: start_chapter,
                verse: Some(start_verse),
            },
            end: PassagePoint {
                chapter: end_chapter,
                verse: Some(end_verse),
            },
        }
    }

    fn is_valid(&self) -> bool {
        if self.book.is_empty()
            || self.start.chapter == 0
            || self.end.chapter == 0
            || self.start.chapter > self.end.chapter
        {
            return false;
        }
        if self.start.chapter == self.end.chapter {
            match (self.start.verse, self.end.verse) {
                (Some(start), Some(end)) => start > 0 && start <= end,
                (None, None) => true,
                _ => false,
            }
        } else {
            matches!(
                (self.start.verse, self.end.verse),
                (Some(start), Some(end)) if start > 0 && end > 0
            ) || matches!((self.start.verse, self.end.verse), (None, None))
        }
    }

    fn includes(&self, verse: &Verse) -> bool {
        let Some((verse_start, verse_end)) = verse_number_span(&verse.number) else {
            return false;
        };
        let after_start = verse.chapter > self.start.chapter
            || (verse.chapter == self.start.chapter
                && self.start.verse.is_none_or(|start| verse_end >= start));
        let before_end = verse.chapter < self.end.chapter
            || (verse.chapter == self.end.chapter
                && self.end.verse.is_none_or(|end| verse_start <= end));
        after_start && before_end
    }
}

/// Verses selected by a passage request.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Passage {
    /// The request that selected these verses.
    pub request: PassageRequest,
    /// Selected verses in USX document order.
    pub verses: Vec<Verse>,
}

impl Passage {
    /// Returns the verse texts joined with a single space.
    pub fn text(&self) -> String {
        self.verses
            .iter()
            .map(|verse| verse.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Writing direction for scripture text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScriptDirection {
    /// Text runs from left to right.
    #[default]
    LeftToRight,
    /// Text runs from right to left.
    RightToLeft,
}

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
    /// A structured request contains invalid or inconsistent endpoints.
    #[error("invalid passage request {request:?}: {reason}")]
    InvalidPassageRequest {
        /// Rejected request.
        request: PassageRequest,
        /// Explanation of the violated request invariant.
        reason: &'static str,
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

#[derive(Default)]
struct Metadata {
    id: Option<String>,
    revision: Option<u32>,
    name: Option<String>,
    local_name: Option<String>,
    abbreviation: Option<String>,
    local_abbreviation: Option<String>,
    scope: Option<String>,
    language_iso: Option<String>,
    ldml: Option<String>,
    script_direction: Option<ScriptDirection>,
    book_names: BTreeMap<String, BookNames>,
    usx_resources: Vec<String>,
}

fn parse_metadata(path: &Path) -> Result<Metadata, Error> {
    let mut reader = Reader::from_reader(BufReader::new(File::open(path)?));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut stack = Vec::<String>::new();
    let mut metadata = Metadata::default();
    let mut current_book = None::<String>;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                let tag = String::from_utf8_lossy(event.local_name().as_ref()).into_owned();
                if stack.is_empty() && tag == "DBLMetadata" {
                    metadata.id = attribute(&reader, &event, b"id")?;
                    metadata.revision = attribute(&reader, &event, b"revision")?
                        .and_then(|value| value.parse().ok());
                } else if tag == "name" && stack.last().is_some_and(|parent| parent == "names") {
                    current_book = attribute(&reader, &event, b"id")?
                        .and_then(|id| id.strip_prefix("book-").map(str::to_ascii_uppercase));
                }
                stack.push(tag);
            }
            Event::Empty(event) => {
                record_empty_element(&reader, &event, &stack, &mut metadata)?;
            }
            Event::Text(text) => {
                let decoded = text.xml_content()?;
                record_text(&stack, &decoded, current_book.as_deref(), &mut metadata);
            }
            Event::GeneralRef(reference) => {
                let reference = reference.decode()?;
                let escaped = format!("&{reference};");
                let value = unescape(&escaped)?.into_owned();
                record_text(&stack, &value, current_book.as_deref(), &mut metadata);
            }
            Event::End(event) => {
                if event.local_name().as_ref() == b"name"
                    && stack
                        .get(stack.len().saturating_sub(2))
                        .is_some_and(|parent| parent == "names")
                {
                    current_book = None;
                }
                stack.pop();
            }
            Event::Eof => break,
            _ => {}
        }
        buffer.clear();
    }

    trim_metadata(&mut metadata);
    Ok(metadata)
}

fn parse_verses(path: &Path, book_code: &str) -> Result<Vec<Verse>, Error> {
    let mut reader = Reader::from_reader(BufReader::new(File::open(path)?));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut verses = Vec::new();
    let mut current = None::<Verse>;
    let mut chapter = None::<u32>;
    let mut excluded_depth = 0_usize;
    let mut version = None::<UsxVersion>;
    let mut found_book = false;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                match event.local_name().as_ref() {
                    b"usx" => version = Some(parse_usx_version(&reader, &event)?),
                    b"book" => {
                        validate_book(&reader, &event, book_code)?;
                        found_book = true;
                    }
                    b"chapter" => return Err(Error::NonEmptyUsxMilestone("chapter")),
                    b"verse" => return Err(Error::NonEmptyUsxMilestone("verse")),
                    _ => {}
                }

                if excluded_depth > 0 {
                    excluded_depth += 1;
                } else if is_excluded(event.local_name().as_ref()) {
                    excluded_depth = 1;
                } else {
                    validate_vid(&reader, &event, current.as_ref())?;
                }
            }
            Event::Empty(event) if excluded_depth == 0 => match event.local_name().as_ref() {
                b"book" => {
                    validate_book(&reader, &event, book_code)?;
                    found_book = true;
                }
                b"chapter" | b"verse" => record_usx_marker(
                    &reader,
                    &event,
                    version.ok_or(Error::MissingUsxField("usx/@version"))?,
                    book_code,
                    &mut chapter,
                    &mut current,
                    &mut verses,
                )?,
                _ => validate_vid(&reader, &event, current.as_ref())?,
            },
            Event::Text(text) if excluded_depth == 0 => {
                if let Some(verse) = &mut current {
                    verse.text.push_str(&text.xml_content()?);
                }
            }
            Event::CData(text) if excluded_depth == 0 => {
                if let Some(verse) = &mut current {
                    verse.text.push_str(&text.decode()?);
                }
            }
            Event::GeneralRef(reference) if excluded_depth == 0 => {
                if let Some(verse) = &mut current {
                    let reference = reference.decode()?;
                    let escaped = format!("&{reference};");
                    verse.text.push_str(&unescape(&escaped)?);
                }
            }
            Event::End(_) if excluded_depth > 0 => excluded_depth -= 1,
            Event::Eof => {
                finish_verse(&mut current, &mut verses);
                break;
            }
            _ => {}
        }
        buffer.clear();
    }

    version.ok_or(Error::MissingUsxField("usx/@version"))?;
    if !found_book {
        return Err(Error::MissingUsxField("book/@code"));
    }
    Ok(verses)
}

#[derive(Clone, Copy)]
struct UsxVersion {
    major: u8,
}

fn parse_usx_book_code(path: &Path) -> Result<String, Error> {
    let mut reader = Reader::from_reader(BufReader::new(File::open(path)?));
    let mut buffer = Vec::new();
    let mut found_version = false;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) | Event::Empty(event) => match event.local_name().as_ref() {
                b"usx" => {
                    parse_usx_version(&reader, &event)?;
                    found_version = true;
                }
                b"book" => {
                    if !found_version {
                        return Err(Error::MissingUsxField("usx/@version"));
                    }
                    validate_style(&reader, &event, "book", "id")?;
                    return attribute(&reader, &event, b"code")?
                        .ok_or(Error::MissingUsxField("book/@code"));
                }
                _ => {}
            },
            Event::Eof => return Err(Error::MissingUsxField("book/@code")),
            _ => {}
        }
        buffer.clear();
    }
}

fn parse_usx_version(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
) -> Result<UsxVersion, Error> {
    let value =
        attribute(reader, event, b"version")?.ok_or(Error::MissingUsxField("usx/@version"))?;
    let mut parts = value.split('.');
    let major = parts.next().and_then(|part| part.parse::<u8>().ok());
    let valid_tail = parts
        .clone()
        .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    if !valid_tail || parts.count() != 1 || !matches!(major, Some(1..=3)) {
        return Err(Error::UnsupportedUsxVersion(value));
    }
    Ok(UsxVersion {
        major: major.unwrap_or_default(),
    })
}

fn validate_book(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    expected: &str,
) -> Result<(), Error> {
    validate_style(reader, event, "book", "id")?;
    let actual = attribute(reader, event, b"code")?.ok_or(Error::MissingUsxField("book/@code"))?;
    if actual != expected {
        return Err(Error::MismatchedBookCode {
            expected: expected.to_owned(),
            actual,
        });
    }
    Ok(())
}

fn validate_style(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    element: &'static str,
    expected: &'static str,
) -> Result<(), Error> {
    let style =
        attribute(reader, event, b"style")?.ok_or(Error::MissingUsxField("element/@style"))?;
    if style != expected {
        return Err(Error::InvalidUsxStyle {
            element,
            expected,
            actual: style,
        });
    }
    Ok(())
}

fn validate_vid(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    current: Option<&Verse>,
) -> Result<(), Error> {
    let Some(actual) = attribute(reader, event, b"vid")? else {
        return Ok(());
    };
    let Some(current) = current else {
        return Err(Error::MismatchedVerseContinuation {
            expected: String::new(),
            actual,
        });
    };
    if actual != current.sid {
        return Err(Error::MismatchedVerseContinuation {
            expected: current.sid.clone(),
            actual,
        });
    }
    Ok(())
}

fn record_usx_marker(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    version: UsxVersion,
    book_code: &str,
    chapter: &mut Option<u32>,
    current: &mut Option<Verse>,
    verses: &mut Vec<Verse>,
) -> Result<(), Error> {
    match event.local_name().as_ref() {
        b"chapter" => {
            finish_verse(current, verses);
            if attribute(reader, event, b"eid")?.is_none() {
                validate_style(reader, event, "chapter", "c")?;
                let number = attribute(reader, event, b"number")?
                    .ok_or(Error::MissingUsxField("chapter/@number"))?;
                *chapter = Some(
                    number
                        .parse()
                        .map_err(|_| Error::InvalidChapterNumber(number))?,
                );
            }
        }
        b"verse" => {
            if let Some(actual) = attribute(reader, event, b"eid")? {
                let Some(open) = current else {
                    return Err(Error::UnexpectedVerseEnd(actual));
                };
                if open.sid != actual {
                    return Err(Error::MismatchedVerseEnd {
                        expected: open.sid.clone(),
                        actual,
                    });
                }
                finish_verse(current, verses);
                return Ok(());
            }

            validate_style(reader, event, "verse", "v")?;
            let number = attribute(reader, event, b"number")?
                .ok_or(Error::MissingUsxField("verse/@number"))?;
            finish_verse(current, verses);
            let chapter_number = chapter.ok_or(Error::MissingUsxField("chapter/@number"))?;
            let sid = match attribute(reader, event, b"sid")? {
                Some(sid) => sid,
                None if version.major < 3 => format!("{book_code} {chapter_number}:{number}"),
                None => return Err(Error::MissingUsxField("verse/@sid")),
            };
            *current = Some(Verse {
                sid,
                chapter: chapter_number,
                number,
                alternate_number: attribute(reader, event, b"altnumber")?,
                published_number: attribute(reader, event, b"pubnumber")?,
                text: String::new(),
            });
        }
        _ => {}
    }
    Ok(())
}

fn finish_verse(current: &mut Option<Verse>, verses: &mut Vec<Verse>) {
    if let Some(mut verse) = current.take() {
        verse.text = verse
            .text
            .split_ascii_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        verses.push(verse);
    }
}

fn is_excluded(name: &[u8]) -> bool {
    matches!(name, b"note" | b"figure" | b"sidebar")
}

fn verse_number_span(number: &str) -> Option<(u32, u32)> {
    let mut values = number.split(['-', ',']).filter_map(leading_number);
    let first = values.next()?;
    Some(values.fold((first, first), |(min, max), value| {
        (min.min(value), max.max(value))
    }))
}

fn leading_number(value: &str) -> Option<u32> {
    let digits = value
        .trim_start_matches(|character: char| {
            character.is_ascii_whitespace() || character == '\u{200f}'
        })
        .chars()
        .take_while(char::is_ascii_digit)
        .collect::<String>();
    (!digits.is_empty()).then(|| digits.parse().ok()).flatten()
}

fn record_text(stack: &[String], value: &str, current_book: Option<&str>, metadata: &mut Metadata) {
    let path = stack.iter().map(String::as_str).collect::<Vec<_>>();
    match path.as_slice() {
        ["DBLMetadata", "identification", "name"] => append(&mut metadata.name, value),
        ["DBLMetadata", "identification", "nameLocal"] => append(&mut metadata.local_name, value),
        ["DBLMetadata", "identification", "abbreviation"] => {
            append(&mut metadata.abbreviation, value)
        }
        ["DBLMetadata", "identification", "abbreviationLocal"] => {
            append(&mut metadata.local_abbreviation, value)
        }
        ["DBLMetadata", "identification", "scope"] => append(&mut metadata.scope, value),
        ["DBLMetadata", "language", "iso"] => append(&mut metadata.language_iso, value),
        ["DBLMetadata", "language", "ldml"] => append(&mut metadata.ldml, value),
        ["DBLMetadata", "language", "scriptDirection"] => {
            metadata.script_direction = Some(if value.trim().eq_ignore_ascii_case("RTL") {
                ScriptDirection::RightToLeft
            } else {
                ScriptDirection::LeftToRight
            });
        }
        ["DBLMetadata", "names", "name", field] => {
            if let Some(code) = current_book {
                let names = metadata.book_names.entry(code.to_owned()).or_default();
                match *field {
                    "abbr" => append(&mut names.abbreviation, value),
                    "short" => append(&mut names.short, value),
                    "long" => append(&mut names.long, value),
                    _ => {}
                }
            }
        }
        _ => {}
    }
}

fn append(destination: &mut Option<String>, value: &str) {
    destination.get_or_insert_with(String::new).push_str(value);
}

fn trim_metadata(metadata: &mut Metadata) {
    for value in [
        &mut metadata.id,
        &mut metadata.name,
        &mut metadata.local_name,
        &mut metadata.abbreviation,
        &mut metadata.local_abbreviation,
        &mut metadata.scope,
        &mut metadata.language_iso,
        &mut metadata.ldml,
    ]
    .into_iter()
    .flatten()
    {
        *value = value.trim().to_owned();
    }
    for names in metadata.book_names.values_mut() {
        for value in [&mut names.abbreviation, &mut names.short, &mut names.long]
            .into_iter()
            .flatten()
        {
            *value = value.trim().to_owned();
        }
    }
}

fn record_empty_element(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    stack: &[String],
    metadata: &mut Metadata,
) -> Result<(), Error> {
    let uri = if event.local_name().as_ref() == b"resource"
        && stack.last().is_some_and(|parent| parent == "manifest")
    {
        attribute(reader, event, b"uri")?
    } else if event.local_name().as_ref() == b"content" {
        attribute(reader, event, b"src")?
    } else {
        None
    };

    if let Some(uri) = uri
        && uri.starts_with("release/USX_")
        && uri.ends_with(".usx")
        && !metadata.usx_resources.contains(&uri)
    {
        metadata.usx_resources.push(uri);
    }
    Ok(())
}

fn attribute(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    name: &[u8],
) -> Result<Option<String>, quick_xml::Error> {
    for attribute in event.attributes().with_checks(false) {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == name {
            return Ok(Some(
                attribute
                    .decode_and_unescape_value(reader.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}

fn required(value: Option<String>, field: &'static str) -> Result<String, Error> {
    value.ok_or(Error::MissingField(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usx_verse_number_sequences() {
        assert_eq!(verse_number_span("2-6a"), Some((2, 6)));
        assert_eq!(verse_number_span("18\u{200f},19"), Some((18, 19)));
        assert_eq!(verse_number_span("3b"), Some((3, 3)));
    }
}
