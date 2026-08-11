# scripture_lib

A Rust reader for unpacked [Digital Bible Library](https://thedigitalbiblelibrary.org/)
bundles. It discovers bundle folders from their `metadata.xml`, indexes their USX
books, and exposes canonicalized locales through ICU4X's `icu_locale::Locale`.

```rust
use scripture_lib::ScriptureLibrary;

let library = ScriptureLibrary::discover(".")?;
let english = library.get("engLXXup").expect("English bundle");
let genesis = english.book("GEN").expect("Genesis");
println!("{}: {}", english.locale, genesis.names.long.as_deref().unwrap_or("GEN"));
# Ok::<(), scripture_lib::Error>(())
```

Run `cargo run -- <folder>` to list the bundles under a folder.
