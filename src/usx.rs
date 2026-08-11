//! Direct parsing of Unified Scripture XML (USX) documents.
//!
//! These functions can be used independently of a Digital Bible Library
//! bundle. [`book_code`](crate::usx::book_code) performs a lightweight read of
//! the document header, while [`verses`](crate::usx::verses) parses its
//! scripture text and validates the expected book code.
//!
//! This module validates the USX structures it consumes, but it is not a full
//! Relax NG validator for every element and style in a USX document.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};

use crate::{Error, Verse};

/// Reads and validates the three-character book code from a USX document.
///
/// The document must declare a supported USX version and a `book` element with
/// the required `id` style and `code` attribute.
pub fn book_code(path: impl AsRef<Path>) -> Result<String, Error> {
    parse_book_code(path.as_ref())
}

/// Parses the verses in a USX document in document order.
///
/// `expected_book_code` is checked against the document's `book/@code` value.
/// USX 3 chapter `sid` and `eid` milestones are required and matched. Verse
/// `eid` milestones are matched when present; a following verse, chapter
/// boundary, or end of file also closes the current verse, allowing documents
/// that omit verse-ending milestones.
pub fn verses(path: impl AsRef<Path>, expected_book_code: &str) -> Result<Vec<Verse>, Error> {
    parse_verses(path.as_ref(), expected_book_code)
}

fn parse_verses(path: &Path, book_code: &str) -> Result<Vec<Verse>, Error> {
    let mut reader = Reader::from_reader(BufReader::new(File::open(path)?));
    reader.config_mut().trim_text(false);
    let mut buffer = Vec::new();
    let mut verses = Vec::new();
    let mut current = None::<Verse>;
    let mut chapter = None::<Chapter>;
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

    let version = version.ok_or(Error::MissingUsxField("usx/@version"))?;
    if !found_book {
        return Err(Error::MissingUsxField("book/@code"));
    }
    if version.major >= 3 && chapter.is_some() {
        return Err(Error::MissingUsxField("chapter/@eid"));
    }
    Ok(verses)
}

#[derive(Clone, Copy)]
struct UsxVersion {
    major: u8,
}

struct Chapter {
    number: u32,
    sid: String,
}

fn parse_book_code(path: &Path) -> Result<String, Error> {
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
    let parts = value.split('.').collect::<Vec<_>>();
    let valid = matches!(parts.len(), 2 | 3)
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
    let major = parts.first().and_then(|part| part.parse::<u8>().ok());
    if !valid || !matches!(major, Some(1..=3)) {
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
    chapter: &mut Option<Chapter>,
    current: &mut Option<Verse>,
    verses: &mut Vec<Verse>,
) -> Result<(), Error> {
    match event.local_name().as_ref() {
        b"chapter" => {
            finish_verse(current, verses);
            if let Some(actual) = attribute(reader, event, b"eid")? {
                let Some(open) = chapter else {
                    return Err(Error::UnexpectedChapterEnd(actual));
                };
                if open.sid != actual {
                    return Err(Error::MismatchedChapterEnd {
                        expected: open.sid.clone(),
                        actual,
                    });
                }
                *chapter = None;
                return Ok(());
            }

            if version.major >= 3 && chapter.is_some() {
                return Err(Error::MissingUsxField("chapter/@eid"));
            }
            validate_style(reader, event, "chapter", "c")?;
            let number = attribute(reader, event, b"number")?
                .ok_or(Error::MissingUsxField("chapter/@number"))?;
            let number: u32 = number
                .parse()
                .map_err(|_| Error::InvalidChapterNumber(number))?;
            let expected_sid = format!("{book_code} {number}");
            let sid = match attribute(reader, event, b"sid")? {
                Some(sid)
                    if version.major < 3
                        || identifier_matches(&sid, book_code, &number.to_string()) =>
                {
                    sid
                }
                Some(actual) => {
                    return Err(Error::InvalidUsxIdentifier {
                        attribute: "chapter/@sid",
                        expected: expected_sid,
                        actual,
                    });
                }
                None if version.major < 3 => expected_sid,
                None => return Err(Error::MissingUsxField("chapter/@sid")),
            };
            *chapter = Some(Chapter { number, sid });
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
            let chapter = chapter
                .as_ref()
                .ok_or(Error::MissingUsxField("chapter/@number"))?;
            let chapter_number = chapter.number;
            let expected_sid = format!("{book_code} {chapter_number}:{number}");
            let sid = match attribute(reader, event, b"sid")? {
                Some(sid)
                    if version.major < 3
                        || identifier_matches(
                            &sid,
                            book_code,
                            &format!("{chapter_number}:{number}"),
                        ) =>
                {
                    sid
                }
                Some(actual) => {
                    return Err(Error::InvalidUsxIdentifier {
                        attribute: "verse/@sid",
                        expected: expected_sid,
                        actual,
                    });
                }
                None if version.major < 3 => expected_sid,
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

fn identifier_matches(actual: &str, book_code: &str, location: &str) -> bool {
    actual
        .strip_prefix(book_code)
        .is_some_and(|suffix| suffix == location || suffix == format!(" {location}"))
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

pub(crate) fn verse_number_span(number: &str) -> Option<(u32, u32)> {
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
