# scripture_lib

A Rust reader for unpacked [Digital Bible Library](https://thedigitalbiblelibrary.org/)
bundles. It discovers bundle folders from their `metadata.xml`, indexes their USX
books, and exposes canonicalized locales through ICU4X's `icu_locale::Locale`.
USX reading follows the official [USFM/USX specification](https://docs.usfm.bible/),
including pre-3 start milestones and USX 3 `sid`, `eid`, and `vid` semantics.

```rust
use scripture_lib::{PassageRequest, ScriptureLibrary};

let library = ScriptureLibrary::discover(".")?;
let english = library.get("engLXXup").expect("English bundle");
let genesis = english.book("GEN").expect("Genesis");
println!("{}: {}", english.locale, genesis.names.long.as_deref().unwrap_or("GEN"));

let request = PassageRequest::verse_range("Genesis", 1, 4, 2, 3)?;
println!("{}", english.passage(&request)?.text());
# Ok::<(), scripture_lib::Error>(())
```

Run `cargo run` to load bundles from `offline`, list them, and open the passage
console. A different bundle directory can be supplied with `cargo run -- <folder>`.

```text
passage> Genesis 1:2-3:4 LXXUP
```

The console accepts whole chapters, chapter ranges, individual verses, verse
ranges, and cross-chapter ranges. Enter `quit` or `exit` to close it.
