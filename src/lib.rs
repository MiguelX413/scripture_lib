//! Reader for unpacked Digital Bible Library (DBL) bundles.

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

    pub fn bundles(&self) -> &[DblBundle] {
        &self.bundles
    }

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
    pub id: String,
    pub revision: Option<u32>,
    pub name: String,
    pub local_name: Option<String>,
    /// Preferred display abbreviation (`abbreviationLocal` when available).
    pub abbreviation: String,
    /// The non-localized `abbreviation` value from DBL metadata.
    pub metadata_abbreviation: String,
    pub local_abbreviation: Option<String>,
    pub scope: Option<String>,
    pub locale: Locale,
    pub script_direction: ScriptDirection,
    books: Vec<Book>,
}

impl DblBundle {
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
                let code =
                    book_code_from_uri(&uri).ok_or_else(|| Error::InvalidBookUri(uri.clone()))?;
                let names = metadata.book_names.get(&code).cloned().unwrap_or_default();
                Ok(Book {
                    code,
                    names,
                    path: root.join(uri),
                })
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

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn books(&self) -> &[Book] {
        &self.books
    }

    pub fn book(&self, code: &str) -> Option<&Book> {
        self.books
            .iter()
            .find(|book| book.code.eq_ignore_ascii_case(code))
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct BookNames {
    pub abbreviation: Option<String>,
    pub short: Option<String>,
    pub long: Option<String>,
}

#[derive(Debug)]
pub struct Book {
    pub code: String,
    pub names: BookNames,
    path: PathBuf,
}

impl Book {
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
}

/// Plain scripture text associated with one USX verse milestone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Verse {
    /// USX scripture reference, such as `GEN 1:1`.
    pub sid: String,
    /// The displayed verse number, which may be a bridge such as `1-2`.
    pub number: String,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScriptDirection {
    #[default]
    LeftToRight,
    RightToLeft,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("could not parse XML: {0}")]
    Xml(#[from] quick_xml::Error),
    #[error("could not decode XML text: {0}")]
    XmlEncoding(#[from] quick_xml::encoding::EncodingError),
    #[error("could not unescape XML text: {0}")]
    XmlEscape(#[from] quick_xml::escape::EscapeError),
    #[error("DBL bundle at {0} has no metadata.xml")]
    MissingMetadata(PathBuf),
    #[error("DBL metadata is missing {0}")]
    MissingField(&'static str),
    #[error("invalid locale {value:?}: {reason}")]
    InvalidLocale { value: String, reason: String },
    #[error("cannot determine a book code from manifest URI {0:?}")]
    InvalidBookUri(String),
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
    let mut chapter = None::<String>;
    let mut excluded_depth = 0_usize;

    loop {
        match reader.read_event_into(&mut buffer)? {
            Event::Start(event) => {
                if excluded_depth > 0 {
                    excluded_depth += 1;
                } else if is_excluded(event.local_name().as_ref()) {
                    excluded_depth = 1;
                } else {
                    record_usx_marker(
                        &reader,
                        &event,
                        book_code,
                        &mut chapter,
                        &mut current,
                        &mut verses,
                    )?;
                }
            }
            Event::Empty(event) if excluded_depth == 0 => {
                record_usx_marker(
                    &reader,
                    &event,
                    book_code,
                    &mut chapter,
                    &mut current,
                    &mut verses,
                )?;
            }
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

    Ok(verses)
}

fn record_usx_marker(
    reader: &Reader<BufReader<File>>,
    event: &BytesStart<'_>,
    book_code: &str,
    chapter: &mut Option<String>,
    current: &mut Option<Verse>,
    verses: &mut Vec<Verse>,
) -> Result<(), Error> {
    match event.local_name().as_ref() {
        b"chapter" => {
            finish_verse(current, verses);
            if let Some(number) = attribute(reader, event, b"number")? {
                *chapter = Some(number);
            }
        }
        b"verse" => {
            if attribute(reader, event, b"eid")?.is_some() {
                finish_verse(current, verses);
                return Ok(());
            }

            if let Some(number) = attribute(reader, event, b"number")? {
                finish_verse(current, verses);
                let sid = attribute(reader, event, b"sid")?.unwrap_or_else(|| {
                    chapter.as_deref().map_or_else(
                        || format!("{book_code} {number}"),
                        |chapter| format!("{book_code} {chapter}:{number}"),
                    )
                });
                *current = Some(Verse {
                    sid,
                    number,
                    text: String::new(),
                });
            }
        }
        _ => {}
    }
    Ok(())
}

fn finish_verse(current: &mut Option<Verse>, verses: &mut Vec<Verse>) {
    if let Some(mut verse) = current.take() {
        verse.text = verse.text.split_whitespace().collect::<Vec<_>>().join(" ");
        verses.push(verse);
    }
}

fn is_excluded(name: &[u8]) -> bool {
    matches!(name, b"note" | b"figure" | b"sidebar")
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
    if event.local_name().as_ref() == b"resource"
        && stack.last().is_some_and(|parent| parent == "manifest")
        && let Some(uri) = attribute(reader, event, b"uri")?
        && uri.starts_with("release/USX_")
        && uri.ends_with(".usx")
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

fn book_code_from_uri(uri: &str) -> Option<String> {
    Path::new(uri)
        .file_stem()?
        .to_str()?
        .split('_')
        .next()
        .map(str::to_owned)
}

fn required(value: Option<String>, field: &'static str) -> Result<String, Error> {
    value.ok_or(Error::MissingField(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_special_book_code_from_filename() {
        assert_eq!(
            book_code_from_uri("release/USX_1/PSA_Psa151inPsa.usx").as_deref(),
            Some("PSA")
        );
    }
}
