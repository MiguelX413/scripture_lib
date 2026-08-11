use std::io::BufRead;

use quick_xml::events::BytesStart;
use quick_xml::{Reader, XmlVersion};

pub(crate) fn attribute<R: BufRead>(
    reader: &Reader<R>,
    event: &BytesStart<'_>,
    name: &[u8],
) -> Result<Option<String>, quick_xml::Error> {
    for attribute in event.attributes().with_checks(false) {
        let attribute = attribute?;
        if attribute.key.local_name().as_ref() == name {
            return Ok(Some(
                attribute
                    .decoded_and_normalized_value(XmlVersion::default(), reader.decoder())?
                    .into_owned(),
            ));
        }
    }
    Ok(None)
}
