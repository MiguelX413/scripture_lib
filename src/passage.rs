use thiserror::Error;

use crate::usx;

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

    pub(crate) fn includes(&self, verse: &Verse) -> bool {
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
