# Entries

Entries are the dictionary's content — each entry represents a word (or lexeme) with its headword, grammatical tags, stems, meanings, inflected forms, etymology, and usage examples.

## Basic Structure

```
entry <name> {
  headword: "<text>"
  tags: [<axis>=<value>, ...]
  stems { <name>: "<value>", ... }
  inflection_class: <name>
  meaning: "<text>"
}
```

| Field | Required | Description |
| ----- | -------- | ----------- |
| `headword` | Yes | The citation form of the word |
| `tags` | No | Classification tags |
| `stems` | No | Stem values for inflection |
| `inflection_class` / `inflect` | No | How the word inflects |
| `meaning` / `meanings` | Yes | Definition(s) |
| `forms_override` | No | Override specific inflected forms |
| `etymology` | No | Etymological information |
| `examples` | No | Usage examples |

## Headword

### Simple Headword

```
entry faren {
  headword: "faren"
  ...
}
```

### Multi-Script Headword

For languages with multiple writing systems (e.g., Japanese with kanji, kana, and romaji):

```
entry taberu {
  headword {
    default: "食べる"
    kana: "たべる"
    romaji: "taberu"
  }
  ...
}
```

The `default` key is used as the primary headword. If no `default` is present, the first script listed is used. All scripts are stored in the `headword_scripts` table in the output database.

## Tags

Tags classify the entry along the axes defined by `tagaxis` declarations:

```
tags: [parts_of_speech=verb, register=formal]
```

An empty tag list is valid:

```
tags: []
```

Tags use `classificatory` and `inflectional` axes. The tag list is a simple `axis=value` list without wildcards (wildcards are only for inflection rules).

## Stems

Stems provide the raw material that inflection templates use to generate word forms:

```
stems { pres: "far", past: "for" }
```

The stem names must match what the entry's inflection class `requires stems`. An entry with no inflection can have empty stems:

```
stems {}
```

### Stem Values in Templates

When an inflection rule contains `{pres}`, the compiler looks up the `pres` key in the entry's stems and substitutes the value. For the entry above, `{pres}` becomes `far`.

## Meaning

### Single Meaning

Most entries have one definition:

```
meaning: "to go, to travel"
```

### Multiple Meanings (Polysemy)

Polysemous words use named meaning blocks:

```
meanings {
  motion   { "to go, to travel (physically)" }
  progress { "to proceed, to advance (abstractly)" }
}
```

Each meaning has an identifier (`motion`, `progress`) that can be used in entry references with the `#` separator:

```
faren#motion      # refers specifically to the "motion" sense
faren#progress    # refers specifically to the "progress" sense
```

## Inflection Specification

An entry can specify its inflection in one of two ways:

### Named Class

Reference a top-level `inflection` declaration:

```
entry faren {
  stems { pres: "far", past: "for" }
  inflection_class: strong_I
  ...
}
```

The stems must satisfy the class's `requires stems`.

### Inline Inflection

Define rules directly in the entry (for one-off irregulars):

```
entry wesen {
  stems {}
  meaning: "to be"

  inflect for {tense, person, number} {
    [tense=present, person=1, number=sg] -> `bin`
    [tense=present, person=2, number=sg] -> `bist`
    [tense=present, person=3, number=sg] -> `is`
    [tense=present, number=pl, _]        -> `sind`
    [tense=past, person=1, number=sg]    -> `was`
    [tense=past, person=2, number=sg]    -> `warst`
    [tense=past, person=3, number=sg]    -> `was`
    [tense=past, number=pl, _]           -> `waren`
    [tense=future, _]                    -> null
  }
}
```

You cannot have both `inflection_class` and `inflect` on the same entry.

### No Inflection

Entries without inflection simply omit both `inflection_class` and `inflect`. They appear in the `entries` table but have no rows in the `forms` table:

```
entry mann {
  headword: "mann"
  tags: [parts_of_speech=noun]
  stems {}
  meaning: "man, person"
}
```

## Forms Override

When using `inflection_class`, you can override specific cells without redefining the entire paradigm:

```
entry faren {
  headword: "faren"
  stems { pres: "far", past: "for" }
  inflection_class: strong_I

  forms_override {
    [tense=future, person=1, number=sg] -> `werde faren`
  }

  meaning: "to go, to travel"
}
```

Override rules are appended to the class's rule set. Because they can be more specific than the class's rules, they win by specificity. This is how you handle individual irregularities without creating a whole new inflection class.

`forms_override` is **not** valid with inline `inflect` — if you're writing rules inline, just write the correct rules directly.

## Etymology

The `etymology` block records a word's history and relationships:

```
etymology {
  proto: "*far-"
  cognates {
    afaran "prefixed derivative"
    far    "nominal root, same PIE origin"
  }
  derived_from: proto_far
  note: "Underwent ablaut in the past tense: a -> o."
}
```

All fields are optional:

| Field | Type | Description |
| ----- | ---- | ----------- |
| `proto` | string | The reconstructed proto-form |
| `cognates` | block | List of cognate entries with notes |
| `derived_from` | entry ref | The entry this word derives from |
| `note` | string | Free-form etymological notes |

### Cognates

Each cognate is an entry reference followed by a string note:

```
cognates {
  afaran "prefixed derivative"
  far    "same PIE root"
}
```

This creates `cognate` links in the output `links` table.

### Derived From

`derived_from` takes an entry reference and creates a directional `derived_from` link:

```
derived_from: proto_far
```

**DAG constraint**: `derived_from` links must form a directed acyclic graph. If entry A derives from B and B derives from A, the compiler reports a cycle error. This prevents circular derivation chains.

## Examples

The `examples` block contains usage examples with glossed tokens:

```
examples {
  example {
    tokens: faren[tense=present, person=1, number=sg] "."
    translation: "I go."
  }
  example {
    tokens: "der" weg[case=nom, number=sg]
    translation: "the way"
  }
}
```

### Token Syntax

A token list is a sequence of:

1. **Entry references** with form specs — the compiler resolves them to the correct inflected form:
   ```
   faren[tense=present, person=1, number=sg]    # resolves to "fare"
   weg[case=nom, number=sg]                      # resolves to "weg"
   ```

2. **String literals** — for function words, punctuation, or any text that doesn't need linking:
   ```
   "der"    # article
   "."      # punctuation
   ```

Entry references with empty brackets `[]` select the headword form.

If a `[form_spec]` matches multiple forms, it's a compile error — the reference must be unambiguous.

### Translation

A free-form string with the example's translation:

```
translation: "I go."
```

## Entry References

Anywhere an entry is referenced (etymology, examples, etc.), the full reference syntax is:

```
[<namespace>.]<entry_id>[#<meaning>][<form_spec>]
```

| Part | Description | Optional? |
| ---- | ----------- | --------- |
| `<namespace>.` | Namespace from `@use ... as` or `@reference ... as` | Yes |
| `<entry_id>` | The entry's identifier | No |
| `#<meaning>` | Selects a specific meaning for polysemous entries | Yes |
| `[<form_spec>]` | Tag conditions selecting a specific inflected form | Yes |

Examples:

```
far                                           # basic reference
far#motion                                    # with meaning
faren[tense=present, person=1, number=sg]     # with form spec
faren#motion[tense=past, number=sg]           # meaning + form
lat.aqua#liquid[case=nom, number=sg]          # namespaced
```

## Complete Entry Example

```
entry faren {
  headword: "faren"
  tags: [parts_of_speech=verb, register=formal]
  stems { pres: "far", past: "for" }
  inflection_class: strong_I

  meanings {
    motion   { "to go, to travel (physically)" }
    progress { "to proceed, to advance (abstractly)" }
  }

  forms_override {
    [tense=future, person=1, number=sg] -> `werde faren`
  }

  etymology {
    proto: "*far-"
    cognates {
      afaran "prefixed derivative"
    }
    note: "Underwent ablaut in the past tense: a -> o."
  }

  examples {
    example {
      tokens: faren[tense=present, person=1, number=sg] "."
      translation: "I go."
    }
  }
}
```
