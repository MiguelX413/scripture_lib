use std::env;
use std::io::{self, Write};
use std::process::ExitCode;

use scripture_lib::{PassageRequest, ScriptureLibrary};

fn main() -> ExitCode {
    let root = env::args_os().nth(1).unwrap_or_else(|| "offline".into());
    let library = match ScriptureLibrary::discover(root) {
        Ok(library) => library,
        Err(error) => {
            eprintln!("error: {error}");
            return ExitCode::FAILURE;
        }
    };

    for bundle in library.bundles() {
        println!(
            "{}\t{}\t{}\t{} books",
            bundle.abbreviation,
            bundle.locale,
            bundle.name,
            bundle.books().len()
        );
    }

    match run_console(&library) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run_console(library: &ScriptureLibrary) -> io::Result<()> {
    let stdin = io::stdin();
    let mut input = String::new();

    loop {
        print!("passage> ");
        io::stdout().flush()?;
        input.clear();
        if stdin.read_line(&mut input)? == 0 {
            println!();
            break;
        }

        let input = input.trim();
        if input.eq_ignore_ascii_case("exit") || input.eq_ignore_ascii_case("quit") {
            break;
        }
        if input.is_empty() {
            continue;
        }

        let query = match parse_query(input) {
            Ok(query) => query,
            Err(error) => {
                eprintln!("error: {error}");
                continue;
            }
        };
        let Some(bundle) = library.get(&query.bundle) else {
            eprintln!("error: bundle {:?} was not found", query.bundle);
            continue;
        };
        match bundle.passage(&query.passage) {
            Ok(passage) => {
                println!("{} {}", bundle.abbreviation, query.passage.book());
                for verse in passage.verses {
                    println!("{}:{}\t{}", verse.chapter, verse.number, verse.text);
                }
            }
            Err(error) => eprintln!("error: {error}"),
        }
    }

    Ok(())
}

#[derive(Debug, Eq, PartialEq)]
struct Query {
    bundle: String,
    passage: PassageRequest,
}

fn parse_query(input: &str) -> Result<Query, String> {
    let mut parts = input.split_whitespace().collect::<Vec<_>>();
    let bundle = parts
        .pop()
        .ok_or_else(|| "missing bundle abbreviation".to_owned())?;
    let reference = parts
        .pop()
        .ok_or_else(|| "missing chapter or verse reference".to_owned())?;
    if parts.is_empty() {
        return Err("missing book name or code".to_owned());
    }
    let book = parts.join(" ");
    let passage = parse_reference(book, reference)?;
    Ok(Query {
        bundle: bundle.to_owned(),
        passage,
    })
}

fn parse_reference(book: String, reference: &str) -> Result<PassageRequest, String> {
    let Some((start, end)) = reference.split_once('-') else {
        let start = parse_point(reference)?;
        return match start.verse {
            Some(verse) => PassageRequest::verse(book, start.chapter, verse),
            None => PassageRequest::chapter(book, start.chapter),
        }
        .map_err(|error| error.to_string());
    };
    if end.contains('-') {
        return Err("a passage can contain only one range separator".to_owned());
    }

    let start = parse_point(start)?;
    if let Some(start_verse) = start.verse {
        if end.contains(':') {
            let end = parse_point(end)?;
            let end_verse = end
                .verse
                .ok_or_else(|| "the ending verse is missing".to_owned())?;
            PassageRequest::verse_range(book, start.chapter, start_verse, end.chapter, end_verse)
                .map_err(|error| error.to_string())
        } else {
            let end_verse = parse_positive_number(end, "verse")?;
            PassageRequest::verses(book, start.chapter, start_verse, end_verse)
                .map_err(|error| error.to_string())
        }
    } else {
        let end = parse_point(end)?;
        if end.verse.is_some() {
            return Err("a chapter range cannot end at a verse".to_owned());
        }
        PassageRequest::chapters(book, start.chapter, end.chapter)
            .map_err(|error| error.to_string())
    }
}

fn parse_point(value: &str) -> Result<PassagePoint, String> {
    let Some((chapter, verse)) = value.split_once(':') else {
        return Ok(PassagePoint {
            chapter: parse_positive_number(value, "chapter")?,
            verse: None,
        });
    };
    if verse.contains(':') {
        return Err("a reference can contain only one colon".to_owned());
    }
    Ok(PassagePoint {
        chapter: parse_positive_number(chapter, "chapter")?,
        verse: Some(parse_positive_number(verse, "verse")?),
    })
}

#[derive(Clone, Copy)]
struct PassagePoint {
    chapter: u32,
    verse: Option<u32>,
}

fn parse_positive_number(value: &str, label: &str) -> Result<u32, String> {
    let number = value
        .parse::<u32>()
        .map_err(|_| format!("invalid {label} number {value:?}"))?;
    if number == 0 {
        return Err(format!("{label} number must be greater than zero"));
    }
    Ok(number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_supported_console_requests() {
        assert_eq!(
            parse_query("Genesis 1 LXXUP").unwrap().passage,
            PassageRequest::chapter("Genesis", 1).unwrap()
        );
        assert_eq!(
            parse_query("Genesis 1-2 LXXUP").unwrap().passage,
            PassageRequest::chapters("Genesis", 1, 2).unwrap()
        );
        assert_eq!(
            parse_query("Genesis 1:4 LXXUP").unwrap().passage,
            PassageRequest::verse("Genesis", 1, 4).unwrap()
        );
        assert_eq!(
            parse_query("Genesis 1:4-5 LXXUP").unwrap().passage,
            PassageRequest::verses("Genesis", 1, 4, 5).unwrap()
        );
        assert_eq!(
            parse_query("Genesis 1:2-3:4 LXXUP").unwrap(),
            Query {
                bundle: "LXXUP".to_owned(),
                passage: PassageRequest::verse_range("Genesis", 1, 2, 3, 4).unwrap(),
            }
        );
    }

    #[test]
    fn rejects_malformed_console_requests() {
        assert!(parse_query("1:2 LXXUP").is_err());
        assert!(parse_query("Genesis 0 LXXUP").is_err());
        assert!(parse_query("Genesis 1-2:3 LXXUP").is_err());
        assert!(parse_query("Genesis 1:2-3-4 LXXUP").is_err());
    }
}
