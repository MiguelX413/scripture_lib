use std::collections::BTreeMap;
use std::io::BufRead;

use quick_xml::Reader;
use quick_xml::escape::unescape;
use quick_xml::events::{BytesStart, Event};

use crate::bundle::{BookNames, ScriptDirection};
use crate::error::Error;
use crate::xml::attribute;

#[derive(Default)]
pub(crate) struct Metadata {
    pub(crate) id: Option<String>,
    pub(crate) revision: Option<u32>,
    pub(crate) name: Option<String>,
    pub(crate) local_name: Option<String>,
    pub(crate) abbreviation: Option<String>,
    pub(crate) local_abbreviation: Option<String>,
    pub(crate) scope: Option<String>,
    pub(crate) language_iso: Option<String>,
    pub(crate) ldml: Option<String>,
    pub(crate) script_direction: Option<ScriptDirection>,
    pub(crate) book_names: BTreeMap<String, BookNames>,
    pub(crate) usx_resources: Vec<String>,
}

pub(crate) fn parse_metadata(source: impl BufRead) -> Result<Metadata, Error> {
    let mut reader = Reader::from_reader(source);
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
                let decoded = text.xml10_content()?;
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

fn record_empty_element<R: BufRead>(
    reader: &Reader<R>,
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

pub(crate) fn required(value: Option<String>, field: &'static str) -> Result<String, Error> {
    value.ok_or(Error::MissingField(field))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_metadata_from_a_buffered_reader() {
        let source = r#"<DBLMetadata id="bundle-id" revision="3">
  <identification>
    <name>Example &amp; Test Bible</name>
    <abbreviation>EXAMPLE</abbreviation>
  </identification>
  <language><ldml>en-US</ldml><scriptDirection>LTR</scriptDirection></language>
  <names><name id="book-gen"><long>Genesis</long></name></names>
  <manifest><resource uri="release/USX_1/GEN.usx" /></manifest>
</DBLMetadata>"#;

        let metadata = parse_metadata(source.as_bytes()).unwrap();
        assert_eq!(metadata.id.as_deref(), Some("bundle-id"));
        assert_eq!(metadata.revision, Some(3));
        assert_eq!(metadata.name.as_deref(), Some("Example & Test Bible"));
        assert_eq!(metadata.ldml.as_deref(), Some("en-US"));
        assert_eq!(
            metadata.script_direction,
            Some(ScriptDirection::LeftToRight)
        );
        assert_eq!(metadata.book_names["GEN"].long.as_deref(), Some("Genesis"));
        assert_eq!(metadata.usx_resources, ["release/USX_1/GEN.usx"]);
    }
}
