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

/// Direct parsing of Unified Scripture XML documents.
pub mod usx;

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
                let code = usx::book_code(&path)?;
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
            .find(|book| book.matches_name(request.book()))
            .ok_or_else(|| Error::BookNotFound(request.book().to_owned()))?;
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
        usx::verses(&self.path, &self.code)
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

/// A chapter and verse location in a passage request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerseReference {
    chapter: u32,
    verse: u32,
}

impl VerseReference {
    /// Returns the one-based chapter number.
    pub fn chapter(self) -> u32 {
        self.chapter
    }

    /// Returns the one-based verse number.
    pub fn verse(self) -> u32 {
        self.verse
    }
}

/// The locations covered by a passage request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PassageRange {
    /// An inclusive range of whole chapters.
    Chapters {
        /// First one-based chapter number.
        start: u32,
        /// Last one-based chapter number.
        end: u32,
    },
    /// An inclusive range of verses, possibly spanning chapters.
    Verses {
        /// First verse in the range.
        start: VerseReference,
        /// Last verse in the range.
        end: VerseReference,
    },
}

/// A validated passage request, including its book name or USX code.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PassageRequest {
    book: String,
    range: PassageRange,
}

impl PassageRequest {
    /// Requests one whole chapter, such as `Genesis 1`.
    pub fn chapter(book: impl Into<String>, chapter: u32) -> Result<Self, InvalidPassageRequest> {
        Self::chapters(book, chapter, chapter)
    }

    /// Requests an inclusive range of whole chapters, such as `Genesis 1-2`.
    pub fn chapters(
        book: impl Into<String>,
        start_chapter: u32,
        end_chapter: u32,
    ) -> Result<Self, InvalidPassageRequest> {
        let book = validate_book_name(book.into())?;
        if start_chapter == 0 || end_chapter == 0 {
            return Err(InvalidPassageRequest::new(
                "chapter numbers must be greater than zero",
            ));
        }
        if start_chapter > end_chapter {
            return Err(InvalidPassageRequest::new(
                "the start chapter must not follow the end chapter",
            ));
        }
        Ok(Self {
            book,
            range: PassageRange::Chapters {
                start: start_chapter,
                end: end_chapter,
            },
        })
    }

    /// Requests one verse, such as `Genesis 1:4`.
    pub fn verse(
        book: impl Into<String>,
        chapter: u32,
        verse: u32,
    ) -> Result<Self, InvalidPassageRequest> {
        Self::verse_range(book, chapter, verse, chapter, verse)
    }

    /// Requests an inclusive verse range in one chapter, such as `Genesis 1:4-5`.
    pub fn verses(
        book: impl Into<String>,
        chapter: u32,
        start_verse: u32,
        end_verse: u32,
    ) -> Result<Self, InvalidPassageRequest> {
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
    ) -> Result<Self, InvalidPassageRequest> {
        let book = validate_book_name(book.into())?;
        if [start_chapter, start_verse, end_chapter, end_verse].contains(&0) {
            return Err(InvalidPassageRequest::new(
                "chapter and verse numbers must be greater than zero",
            ));
        }
        if (start_chapter, start_verse) > (end_chapter, end_verse) {
            return Err(InvalidPassageRequest::new(
                "the start verse must not follow the end verse",
            ));
        }
        Ok(Self {
            book,
            range: PassageRange::Verses {
                start: VerseReference {
                    chapter: start_chapter,
                    verse: start_verse,
                },
                end: VerseReference {
                    chapter: end_chapter,
                    verse: end_verse,
                },
            },
        })
    }

    /// Returns the requested USX book code, name, or abbreviation.
    pub fn book(&self) -> &str {
        &self.book
    }

    /// Returns the validated range covered by this request.
    pub fn range(&self) -> PassageRange {
        self.range
    }

    fn includes(&self, verse: &Verse) -> bool {
        let Some((verse_start, verse_end)) = usx::verse_number_span(&verse.number) else {
            return false;
        };
        match self.range {
            PassageRange::Chapters { start, end } => (start..=end).contains(&verse.chapter),
            PassageRange::Verses { start, end } => {
                (verse.chapter, verse_end) >= (start.chapter, start.verse)
                    && (verse.chapter, verse_start) <= (end.chapter, end.verse)
            }
        }
    }
}

fn validate_book_name(book: String) -> Result<String, InvalidPassageRequest> {
    if book.trim().is_empty() {
        Err(InvalidPassageRequest::new(
            "the book name must not be empty",
        ))
    } else {
        Ok(book)
    }
}

/// The reason a passage request could not be constructed.
#[derive(Clone, Copy, Debug, Eq, Error, PartialEq)]
#[error("invalid passage request: {reason}")]
pub struct InvalidPassageRequest {
    reason: &'static str,
}

impl InvalidPassageRequest {
    fn new(reason: &'static str) -> Self {
        Self { reason }
    }

    /// Returns the violated request invariant.
    pub fn reason(self) -> &'static str {
        self.reason
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
