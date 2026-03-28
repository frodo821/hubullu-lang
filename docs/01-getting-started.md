# Getting Started with Hubullu

Hubullu is a compiler for **LexDSL**, a domain-specific language for describing dictionaries of artificial natural languages (constructed languages). It compiles `.hu` source files into a SQLite database containing entries, inflected forms, inter-entry links, and metadata with full-text search.

## Design Principles

- **Language-neutral**: supports any morphological type — fusional, agglutinative, templatic, isolating
- **Separation of declaration and logic**: entries hold data; inflection logic lives in reusable paradigm definitions
- **Compile-time integrity**: broken references, missing inflections, and ambiguous rules are caught before any output is produced
- **Spelling-change safety**: entries are referenced by stable IDs, so renaming a headword never cascades

## Installation

Build from source (requires Rust 1.70+):

```sh
cargo build --release
```

The binary is at `target/release/hubullu`.

## CLI Usage

```sh
hubullu <input.hu> [-o <output.sqlite>]
```

| Argument      | Description                                   |
| ------------- | --------------------------------------------- |
| `<input.hu>` | Entry-point `.hu` file (required, positional) |
| `-o, --output` | Output SQLite path (default: `dictionary.sqlite`) |

On success, prints `Compiled to <output>` to stderr. On failure, prints diagnostics and exits with code 1.

## Your First Project

Create this directory structure:

```
my-lang/
  profile.hu       # tag axes, axis values, inflection classes
  main.hu          # entry-point file with entries
```

### Step 1: Define the profile

`profile.hu` — declares the grammatical categories and inflection patterns:

```
# Declare a tag axis for parts of speech
tagaxis parts_of_speech {
  role: classificatory
  display: { en: "Part of Speech" }
  index: exact
}

# Declare inflectional axes
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

tagaxis number {
  role: inflectional
  display: { en: "Number" }
}

# Add values to the axes
@extend pos_values for tagaxis parts_of_speech {
  verb { display: { en: "Verb" } }
  noun { display: { en: "Noun" } }
}

@extend tense_values for tagaxis tense {
  present { display: { en: "Present" } }
  past    { display: { en: "Past" } }
}

@extend number_values for tagaxis number {
  sg { display: { en: "Singular" } }
  pl { display: { en: "Plural" } }
}

# Define an inflection class
inflection regular_verb for {tense, number} {
  requires stems: root

  [tense=present, number=sg] -> `{root}s`
  [tense=present, number=pl] -> `{root}`
  [tense=past, number=sg]    -> `{root}ed`
  [tense=past, number=pl]    -> `{root}ed`
}
```

### Step 2: Write entries

`main.hu` — imports the profile, defines dictionary entries:

```
@use * from "profile.hu"

entry walk {
  headword: "walk"
  tags: [parts_of_speech=verb]
  stems { root: "walk" }
  inflection_class: regular_verb
  meaning: "to move on foot"
}

entry house {
  headword: "house"
  tags: [parts_of_speech=noun]
  stems {}
  meaning: "a building for habitation"
}
```

### Step 3: Compile

```sh
hubullu main.hu -o my-lang.sqlite
```

### Step 4: Query the result

```sh
sqlite3 my-lang.sqlite "SELECT * FROM entries;"
sqlite3 my-lang.sqlite "SELECT * FROM forms;"
```

You'll see:
- Two entries (`walk` and `house`)
- Four inflected forms for `walk`: `walks`, `walk`, `walked`, `walked` — each tagged with their tense and number

## What's Next

| Topic | Guide |
| ----- | ----- |
| Core language concepts | [Language Basics](02-language-basics.md) |
| Tag axes and axis values | [Tag Axes & @extend](03-tagaxes-and-extends.md) |
| Inflection paradigms | [Inflection](04-inflection.md) |
| Entry structure | [Entries](05-entries.md) |
| Multi-file projects | [Imports](06-imports.md) |
| Template system | [Templates](07-templates.md) |
| Complete worked examples | [Real-World Examples](08-real-world-examples.md) |
| Full syntax & error catalog | [Reference](09-reference.md) |
