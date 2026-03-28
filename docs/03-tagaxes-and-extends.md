# Tag Axes & @extend

Tag axes are the foundation of LexDSL's type system. They define the grammatical dimensions (tense, case, number, gender, etc.) and classification categories (part of speech, register, etc.) that structure your dictionary.

## Declaring a Tag Axis

```
tagaxis <name> {
  role: <role>
  display: { <lang>: "<text>", ... }
  index: <index_kind>               # optional
}
```

### Fields

| Field | Required | Description |
| ----- | -------- | ----------- |
| `role` | Yes | One of `inflectional`, `classificatory`, `structural` |
| `display` | No | Multilingual display names as `{ lang_code: "text" }` pairs |
| `index` | No | `exact` or `fulltext` (only meaningful for classificatory axes) |

### Roles

The `role` determines how the axis participates in the compilation:

#### `inflectional`

Axes that define dimensions of conjugation or declension. These can be used in `inflection ... for { }` declarations and automatically generate form index entries.

```
tagaxis tense {
  role: inflectional
  display: { en: "Tense", ja: "時制" }
}

tagaxis case {
  role: inflectional
  display: { en: "Case" }
}

tagaxis number {
  role: inflectional
  display: { en: "Number" }
}
```

#### `classificatory`

Axes used for classification and search — not for inflection. Entries can be tagged with these values, and you can optionally index them for efficient queries.

```
tagaxis parts_of_speech {
  role: classificatory
  display: { en: "Part of Speech" }
  index: exact
}

tagaxis register {
  role: classificatory
  display: { en: "Register" }
}
```

#### `structural`

Axes that describe structural metadata about stems — for example, the root type in Semitic languages. Structural axes support `slots` declarations on their values (see below).

```
tagaxis stem_type {
  role: structural
  display: { en: "Stem Type" }
}
```

### Index Kinds

| Value | Effect |
| ----- | ------ |
| `exact` | Creates an exact-match index on the axis |
| `fulltext` | Creates a full-text search index |
| (omitted) | No index |

Inflectional axes automatically get form indexes — you don't need `index` for them. The `headword` and `meaning` fields are always indexed.

## Adding Values with @extend

A `tagaxis` declaration creates the axis, but it has no values until you add them with `@extend`:

```
@extend <name> for tagaxis <axis_name> {
  <value> { display: { <lang>: "<text>", ... } }
  <value> { display: { <lang>: "<text>", ... } }
  ...
}
```

The `<name>` is a unique identifier for this extension — it's what you use in `@use` to import it.

### Basic Example

```
tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

@extend tense_values for tagaxis tense {
  present { display: { en: "Present" } }
  past    { display: { en: "Past" } }
  future  { display: { en: "Future" } }
}
```

After this, the `tense` axis has three values: `present`, `past`, `future`.

### Numeric Values

Digits are valid identifiers, so numeric tag values work naturally:

```
tagaxis person {
  role: inflectional
  display: { en: "Person" }
}

@extend person_values for tagaxis person {
  1 { display: { en: "1st" } }
  2 { display: { en: "2nd" } }
  3 { display: { en: "3rd" } }
}
```

### Multilingual Display Names

Display names support any number of language codes:

```
@extend basic_pos for tagaxis parts_of_speech {
  verb { display: { ja: "動詞", en: "Verb", de: "Verb" } }
  noun { display: { ja: "名詞", en: "Noun", de: "Substantiv" } }
  adj  { display: { ja: "形容詞", en: "Adjective", de: "Adjektiv" } }
}
```

### Structural Slots

Values on `structural` axes can declare named **slots** — positions within a stem that can be individually referenced in templates:

```
@extend stem_types for tagaxis stem_type {
  consonantal_3 {
    display: { en: "Triconsonantal Root" }
    slots: [C1, C2, C3]
  }
  consonantal_4 {
    display: { en: "Quadriconsonantal Root" }
    slots: [C1, C2, C3, C4]
  }
}
```

These slots are then used in templates as `{stem.slot}`:

```
inflection form_I for {tense, person, number} {
  requires stems: root[stem_type=consonantal_3]

  [tense=perfect, person=3, number=sg] -> `{root.C1}a{root.C2}a{root.C3}a`
}
```

See [Templates](07-templates.md) for details on structural slot interpolation.

## Scoping Rules

`@extend` declarations are **scoped** — they only take effect in files where they have been imported via `@use`. This prevents unexpected global side effects.

```
# file: extensions.hu
@extend extra_tenses for tagaxis tense {
  pluperfect { display: { en: "Pluperfect" } }
}

# file: verbs.hu
@use extra_tenses from "extensions.hu"
# Now tense has the pluperfect value IN THIS FILE

# file: nouns.hu
# tense does NOT have pluperfect here (not imported)
```

With glob imports (`@use * from "..."`), all `@extend` declarations from the imported file are included.

## Conflict Rules

The compiler enforces deterministic behavior by disallowing conflicts:

| Scenario | Result |
| -------- | ------ |
| Adding a new value to an axis | Allowed (merge) |
| Two `@extend`s adding the same value to the same axis | Compile error |
| Redefining an existing value | Compile error |
| Overwriting axis fields (`role`, `display`) | Compile error |

Because only additions are allowed (never redefinitions), the order of `@use` imports doesn't matter — the result is always the same.

## Splitting Extensions Across Files

You can spread `@extend`s across multiple files for organization. Each must have a unique name and add unique values:

```
# core_pos.hu
@extend core_pos for tagaxis parts_of_speech {
  verb { display: { en: "Verb" } }
  noun { display: { en: "Noun" } }
}

# extra_pos.hu
@extend extra_pos for tagaxis parts_of_speech {
  adj  { display: { en: "Adjective" } }
  adv  { display: { en: "Adverb" } }
}

# main.hu
@use * from "core_pos.hu"
@use * from "extra_pos.hu"
# parts_of_speech now has: verb, noun, adj, adv
```

## Complete Example

A profile for a language with verbs, nouns, and adjectives:

```
# --- Axes ---

tagaxis pos {
  role: classificatory
  display: { en: "Part of Speech" }
  index: exact
}

tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

tagaxis number {
  role: inflectional
  display: { en: "Number" }
}

tagaxis person {
  role: inflectional
  display: { en: "Person" }
}

tagaxis case {
  role: inflectional
  display: { en: "Case" }
}

tagaxis gender {
  role: inflectional
  display: { en: "Gender" }
}

# --- Values ---

@extend pos_vals for tagaxis pos {
  verb { display: { en: "Verb" } }
  noun { display: { en: "Noun" } }
  adj  { display: { en: "Adjective" } }
}

@extend tense_vals for tagaxis tense {
  present { display: { en: "Present" } }
  past    { display: { en: "Past" } }
}

@extend number_vals for tagaxis number {
  sg { display: { en: "Singular" } }
  pl { display: { en: "Plural" } }
}

@extend person_vals for tagaxis person {
  1 { display: { en: "1st" } }
  2 { display: { en: "2nd" } }
  3 { display: { en: "3rd" } }
}

@extend case_vals for tagaxis case {
  nom { display: { en: "Nominative" } }
  acc { display: { en: "Accusative" } }
  dat { display: { en: "Dative" } }
  gen { display: { en: "Genitive" } }
}

@extend gender_vals for tagaxis gender {
  masc { display: { en: "Masculine" } }
  fem  { display: { en: "Feminine" } }
  neut { display: { en: "Neuter" } }
}
```
