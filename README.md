# hubullu

A compiler for the **Hubullu** language (`.hu`), a domain-specific language for describing artificial natural language dictionaries. It compiles `.hu` source files into a SQLite database containing entries, inflected forms, links, and full-text search indices.

## What is Hubullu?

Hubullu lets you declaratively define dictionary entries for constructed (or natural) languages, including:

- **Tag axes** — grammatical dimensions (part of speech, tense, number, etc.)
- **Inflection paradigms** — rule-based form generation with stem interpolation
- **Entries** — headwords with tags, stems, meanings, examples, and etymology
- **Multi-file projects** — `@use` for imports, `@reference` for cross-file entry access

Example:

```
tagaxis parts_of_speech {
  role: classificatory
  display: { en: "Part of Speech" }
  index: exact
}

inflection strong_I for {tense, number} {
  requires stems: pres, past
  [tense=present, number=sg] -> `{pres}s`
  [tense=present, number=pl] -> `{pres}`
  [tense=past, number=sg]    -> `{past}`
  [tense=past, number=pl]    -> `{past}en`
}

entry faren {
  headword: "faren"
  tags: [parts_of_speech=verb]
  stems { pres: "far", past: "for" }
  inflection_class: strong_I
  meaning: "to go"
}
```

## Installation

Requires Rust 1.70+.

```sh
cargo install --git https://github.com/frodo821/hubullu-lang.git
```

## Usage

### Compile

Compile a `.hu` project to a SQLite database:

```sh
hubullu compile main.hu -o dictionary.huc
```

### Lint

Check `.hu` files for warnings and style issues:

```sh
hubullu lint main.hu
hubullu lint main.hu --fix   # auto-fix where possible
```

### Render

Render a `.hut` token list against a compiled database:

```sh
hubullu render tokens.hut
```

### LSP

Start a Language Server Protocol server (for editor integration):

```sh
hubullu lsp
```

## Examples

The `examples/` directory contains complete dictionary projects for several languages:

- `arabic/` — Arabic
- `chinese/` — Chinese
- `dutch/` — Dutch
- `english/` — English
- `latin/` — Latin
- `turkish/` — Turkish

## Library usage

hubullu can also be used as a Rust library:

```rust
use std::path::Path;

// Full compilation
hubullu::compile(Path::new("main.hu"), Path::new("dict.huc"))
    .expect("compilation failed");

// Parse only (for tooling)
let result = hubullu::parse_source(r#"entry foo { headword: "foo" }"#, "test.hu");
assert!(!result.has_errors());
```

## License

MIT
