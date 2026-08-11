use std::fs;
use std::path::PathBuf;

use scripture_lib::{ScriptDirection, ScriptureLibrary};

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
  </names>
  <manifest>
    <resource mimeType="application/xml" uri="release/USX_1/GEN.usx" />
    <resource mimeType="application/xml" uri="release/styles.xml" />
  </manifest>
</DBLMetadata>"#,
    )
    .unwrap();
    fs::write(
        directory.0.join("example/release/USX_1/GEN.usx"),
        r#"<usx version="3.0">
  <book code="GEN" style="id">Genesis</book>
  <chapter number="1" style="c" sid="GEN 1" />
  <para style="p">
    <verse number="1" style="v" sid="GEN 1:1" />First &amp; <char style="add">added</char>.<note style="f">Ignored note</note>
    <verse number="2" style="v" sid="GEN 1:2" />Second.<verse eid="GEN 1:2" />
    <verse number="3" style="v" sid="GEN 1:3" />Third, with no end.
    <verse number="4" style="v" />Fourth, with neither milestone.
  </para>
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
    assert_eq!(bundle.books().len(), 1);

    let genesis = bundle.book("gen").unwrap();
    assert_eq!(genesis.names.long.as_deref(), Some("Genesis"));
    assert!(genesis.read_usx().unwrap().contains("<usx"));

    let verses = genesis.verses().unwrap();
    assert_eq!(verses.len(), 4);
    assert_eq!(verses[0].sid, "GEN 1:1");
    assert_eq!(verses[0].text, "First & added.");
    assert_eq!(verses[1].text, "Second.");
    assert_eq!(verses[2].text, "Third, with no end.");
    assert_eq!(verses[3].sid, "GEN 1:4");
    assert_eq!(verses[3].text, "Fourth, with neither milestone.");
}
