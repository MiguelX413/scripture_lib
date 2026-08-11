use std::fs;
use std::path::PathBuf;

use scripture_lib::{Error, PassageRange, PassageRequest, ScriptDirection, ScriptureLibrary, usx};

struct TestDirectory(PathBuf);

impl TestDirectory {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("scripture-lib-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(path.join("example/release/USX_1")).unwrap();
        Self(path)
    }
}

impl Drop for TestDirectory {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn passage_requests_are_valid_by_construction() {
    assert!(PassageRequest::chapter("", 1).is_err());
    assert!(PassageRequest::chapter("Genesis", 0).is_err());
    assert!(PassageRequest::chapters("Genesis", 2, 1).is_err());
    assert!(PassageRequest::verse("Genesis", 1, 0).is_err());
    assert!(PassageRequest::verses("Genesis", 1, 5, 4).is_err());
    assert!(PassageRequest::verse_range("Genesis", 2, 1, 1, 10).is_err());

    let request = PassageRequest::verse_range("Genesis", 1, 4, 2, 3).unwrap();
    assert_eq!(request.book(), "Genesis");
    let PassageRange::Verses { start, end } = request.range() else {
        panic!("expected a verse range");
    };
    assert_eq!((start.chapter(), start.verse()), (1, 4));
    assert_eq!((end.chapter(), end.verse()), (2, 3));
}

#[test]
fn discovers_and_reads_a_dbl_folder() {
    let directory = TestDirectory::new();
    fs::write(
        directory.0.join("example/metadata.xml"),
        r#"<?xml version="1.0" encoding="utf-8"?>
<DBLMetadata id="bundle-id" revision="3">
  <identification>
    <name>Example &amp; Test Bible</name>
    <nameLocal>Example Bible</nameLocal>
    <abbreviation>EXAMPLE</abbreviation>
    <abbreviationLocal>LOCAL</abbreviationLocal>
    <scope>New Testament</scope>
  </identification>
  <language>
    <iso>eng</iso>
    <ldml>en-US</ldml>
    <scriptDirection>LTR</scriptDirection>
  </language>
  <names>
    <name id="book-gen"><abbr>Gen.</abbr><short>Genesis</short><long>Genesis</long></name>
    <name id="book-exo"><abbr>Ex.</abbr><short>Exodus</short><long>Exodus</long></name>
  </names>
  <manifest>
    <resource mimeType="application/xml" uri="scripture.usx" />
    <resource mimeType="application/xml" uri="release/styles.xml" />
  </manifest>
  <publications>
    <publication id="p1" default="true">
      <structure>
        <content src="release/USX_1/scripture.usx" role="GEN" />
        <content src="release/USX_1/EXO.usx" role="EXO" />
      </structure>
    </publication>
  </publications>
</DBLMetadata>"#,
    )
    .unwrap();
    fs::write(
        directory.0.join("example/release/USX_1/scripture.usx"),
        r#"<usx version="2.6">
  <book code="GEN" style="id">Genesis</book>
  <chapter number="1" style="c" />
  <para style="p">
    <verse number="1" style="v" />First &amp; <char style="add">added</char>.<note style="f">Ignored note</note>
    <verse number="2" style="v" />Second.
    <verse number="3" style="v" />Third, with no end.
    <verse number="4" style="v" />Fourth, with neither milestone.
    <verse number="5-6" style="v" />Fifth and sixth.
  </para>
  <chapter number="2" style="c" />
  <para style="p">
    <verse number="1" style="v" />Chapter two, first.
    <verse number="2" style="v" />Chapter two, second.
    <verse number="3" style="v" />Chapter two, third.
  </para>
</usx>"#,
    )
    .unwrap();
    fs::write(
        directory.0.join("example/release/USX_1/EXO.usx"),
        r#"<usx version="3.0">
  <book code="EXO" style="id" />
  <chapter number="1" style="c" sid="EXO 1" />
  <para style="p">
    <verse number="1" altnumber="1a" pubnumber="I" style="v" sid="EXO 1:1" />Official
    <char style="add">verse</char> text
  </para>
  <para style="q1" vid="EXO 1:1">continues here.<note caller="+" style="f"><char style="ft">Not verse text.</char></note>
    <verse eid="EXO 1:1" />
    <verse number="2" style="v" sid="EXO 1:2" />Recovered missing end.
  </para>
  <chapter eid="EXO 1" />
</usx>"#,
    )
    .unwrap();
    let patch_version_path = directory.0.join("patch-version.usx");
    fs::write(
        &patch_version_path,
        r#"<usx version="3.0.8">
  <book code="MAT" style="id" />
  <para style="mt1">Matthew</para>
  <chapter number="01" style="c" sid="MAT 01"></chapter>
  <para style="p"><verse number="1{RLM}-2" style="v" sid="MAT 01:1-2"></verse>Text</para>
  <para style="q" vid="MAT01:1-2">continued.<verse eid="MAT01:1-2"></verse></para>
  <chapter eid="MAT01"></chapter>
</usx>"#
            .replace("{RLM}", "\u{200f}"),
    )
    .unwrap();
    let missing_chapter_sid_path = directory.0.join("missing-chapter-sid.usx");
    fs::write(
        &missing_chapter_sid_path,
        r#"<usx version="3.1">
  <book code="MAT" style="id" />
  <chapter number="1" style="c" />
</usx>"#,
    )
    .unwrap();
    let mismatched_chapter_end_path = directory.0.join("mismatched-chapter-end.usx");
    fs::write(
        &mismatched_chapter_end_path,
        r#"<usx version="3.1">
  <book code="MAT" style="id" />
  <chapter number="1" style="c" sid="MAT 1" />
  <chapter eid="MAT 2" />
</usx>"#,
    )
    .unwrap();

    let library = ScriptureLibrary::discover(&directory.0).unwrap();
    assert_eq!(library.bundles().len(), 1);

    let bundle = library.get("local").unwrap();
    assert_eq!(bundle.name, "Example & Test Bible");
    assert_eq!(bundle.abbreviation, "LOCAL");
    assert_eq!(bundle.metadata_abbreviation, "EXAMPLE");
    assert_eq!(bundle.local_abbreviation.as_deref(), Some("LOCAL"));
    assert!(library.get("example").is_some());
    assert_eq!(bundle.locale.to_string(), "en-US");
    assert_eq!(bundle.script_direction, ScriptDirection::LeftToRight);
    assert_eq!(bundle.books().len(), 2);

    let genesis = bundle.book("gen").unwrap();
    assert_eq!(genesis.names.long.as_deref(), Some("Genesis"));
    assert!(genesis.read_usx().unwrap().contains("<usx"));
    assert_eq!(usx::book_code(genesis.path()).unwrap(), "GEN");
    assert_eq!(usx::book_code(&patch_version_path).unwrap(), "MAT");
    let patch_version_verses = usx::verses(&patch_version_path, "MAT").unwrap();
    assert_eq!(patch_version_verses.len(), 1);
    assert_eq!(patch_version_verses[0].chapter, 1);
    assert_eq!(patch_version_verses[0].number, "1\u{200f}-2");
    assert_eq!(patch_version_verses[0].text, "Text continued.");
    assert!(matches!(
        usx::verses(&missing_chapter_sid_path, "MAT"),
        Err(Error::MissingUsxField("chapter/@sid"))
    ));
    assert!(matches!(
        usx::verses(&mismatched_chapter_end_path, "MAT"),
        Err(Error::MismatchedChapterEnd { .. })
    ));

    let verses = genesis.verses().unwrap();
    assert_eq!(usx::verses(genesis.path(), "GEN").unwrap(), verses);
    assert_eq!(verses.len(), 8);
    assert_eq!(verses[0].sid, "GEN 1:1");
    assert_eq!(verses[0].text, "First & added.");
    assert_eq!(verses[1].text, "Second.");
    assert_eq!(verses[2].text, "Third, with no end.");
    assert_eq!(verses[3].sid, "GEN 1:4");
    assert_eq!(verses[3].text, "Fourth, with neither milestone.");

    let official = bundle.book("EXO").unwrap().verses().unwrap();
    assert_eq!(official.len(), 2);
    assert_eq!(official[0].alternate_number.as_deref(), Some("1a"));
    assert_eq!(official[0].published_number.as_deref(), Some("I"));
    assert_eq!(official[0].text, "Official verse text continues here.");
    assert_eq!(official[1].text, "Recovered missing end.");

    assert_eq!(
        bundle
            .passage(&PassageRequest::chapter("Genesis", 1).unwrap())
            .unwrap()
            .verses
            .len(),
        5
    );
    assert_eq!(
        bundle
            .passage(&PassageRequest::chapters("Genesis", 1, 2).unwrap())
            .unwrap()
            .verses
            .len(),
        8
    );
    assert_eq!(
        bundle
            .passage(&PassageRequest::verse("GEN", 1, 4).unwrap())
            .unwrap()
            .text(),
        "Fourth, with neither milestone."
    );
    assert_eq!(
        bundle
            .passage(&PassageRequest::verses("Genesis", 1, 4, 5).unwrap())
            .unwrap()
            .verses
            .len(),
        2
    );
    assert_eq!(
        bundle
            .passage(&PassageRequest::verse("Genesis", 1, 6).unwrap())
            .unwrap()
            .verses[0]
            .number,
        "5-6"
    );
    let cross_chapter = bundle
        .passage(&PassageRequest::verse_range("Genesis", 1, 4, 2, 3).unwrap())
        .unwrap();
    assert_eq!(cross_chapter.verses.len(), 5);
    assert_eq!(cross_chapter.verses.first().unwrap().sid, "GEN 1:4");
    assert_eq!(cross_chapter.verses.last().unwrap().sid, "GEN 2:3");
}
