use std::fs;
use std::path::{Path, PathBuf};

use icu_locale::{Locale, LocaleCanonicalizer};

use crate::error::Error;
use crate::metadata::{parse_metadata, required};
use crate::passage::{Passage, PassageRequest, Verse};
use crate::usx;

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

/// Writing direction for scripture text.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScriptDirection {
    /// Text runs from left to right.
    #[default]
    LeftToRight,
    /// Text runs from right to left.
    RightToLeft,
}
