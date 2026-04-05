# Real-World Examples

This guide walks through complete, realistic Hubullu projects for different morphological types. Each example shows how to model a specific kind of language from scratch.

## Example 1: A Germanic Conlang (Fusional)

A fusional language where verbs have ablaut (stem vowel changes) and adjectives agree in gender, case, and number. This mirrors the comprehensive fixture included with Hubullu.

### Project Layout

```
aldisch/
  profile.hu
  verbs.hu
  nouns.hu
  adjectives.hu
```

### profile.hu

```
# ============================================================
# Aldisch — a fictional Germanic language
# ============================================================

# --- Tag Axes ---

tagaxis pos {
  role: classificatory
  display: { en: "Part of Speech", de: "Wortart" }
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

tagaxis register {
  role: classificatory
  display: { en: "Register" }
}

# --- Axis Values ---

@extend pos_vals for tagaxis pos {
  verb { display: { en: "Verb" } }
  noun { display: { en: "Noun" } }
  adj  { display: { en: "Adjective" } }
}

@extend tense_vals for tagaxis tense {
  present { display: { en: "Present" } }
  past    { display: { en: "Past" } }
  future  { display: { en: "Future" } }
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

@extend register_vals for tagaxis register {
  formal   { display: { en: "Formal" } }
  informal { display: { en: "Informal" } }
}

# --- Verb Inflection: Strong Class I (with ablaut) ---
# Verbs provide two stems: pres (present tense) and past (past tense)
# The vowel alternation is encoded in the stems, not the rules

inflection strong_I for {tense, person, number} {
  requires stems: pres, past

  [tense=present, person=1, number=sg] -> `{pres}e`
  [tense=present, person=2, number=sg] -> `{pres}est`
  [tense=present, person=3, number=sg] -> `{pres}eth`
  [tense=present, number=pl, _]        -> `{pres}en`
  [tense=past,    person=1, number=sg] -> `{past}`
  [tense=past,    person=2, number=sg] -> `{past}e`
  [tense=past,    person=3, number=sg] -> `{past}`
  [tense=past,    number=pl, _]        -> `{past}en`
  [tense=future,  _]                   -> null
}

# --- Verb Inflection: Weak (regular suffixation) ---

inflection weak for {tense, person, number} {
  requires stems: root

  [tense=present, person=1, number=sg] -> `{root}e`
  [tense=present, person=2, number=sg] -> `{root}est`
  [tense=present, person=3, number=sg] -> `{root}et`
  [tense=present, number=pl, _]        -> `{root}en`
  [tense=past,    person=1, number=sg] -> `{root}te`
  [tense=past,    person=2, number=sg] -> `{root}test`
  [tense=past,    person=3, number=sg] -> `{root}te`
  [tense=past,    number=pl, _]        -> `{root}ten`
  [tense=future,  _]                   -> null
}

# --- Noun Declension ---

inflection strong_noun for {case, number} {
  requires stems: root

  [case=nom, number=sg] -> `{root}`
  [case=acc, number=sg] -> `{root}`
  [case=dat, number=sg] -> `{root}e`
  [case=gen, number=sg] -> `{root}es`
  [case=nom, number=pl] -> `{root}e`
  [case=acc, number=pl] -> `{root}e`
  [case=dat, number=pl] -> `{root}en`
  [case=gen, number=pl] -> `{root}e`
}

# --- Adjective Declension (delegation by gender) ---

inflection adj_strong for {case, number, gender} {
  requires stems: root

  [gender=masc, _] -> adj_masc[case, number] with stems { root: root }
  [gender=fem,  _] -> adj_fem[case, number]  with stems { root: root }
  [gender=neut, _] -> adj_neut[case, number] with stems { root: root }
}

inflection adj_masc for {case, number} {
  requires stems: root
  [case=nom, number=sg] -> `{root}er`
  [case=acc, number=sg] -> `{root}en`
  [case=dat, number=sg] -> `{root}em`
  [case=gen, number=sg] -> `{root}es`
  [case=nom, number=pl] -> `{root}e`
  [case=acc, number=pl] -> `{root}e`
  [case=dat, number=pl] -> `{root}en`
  [case=gen, number=pl] -> `{root}er`
}

inflection adj_fem for {case, number} {
  requires stems: root
  [case=nom, number=sg] -> `{root}e`
  [case=acc, number=sg] -> `{root}e`
  [case=dat, number=sg] -> `{root}er`
  [case=gen, number=sg] -> `{root}er`
  [case=nom, number=pl] -> `{root}e`
  [case=acc, number=pl] -> `{root}e`
  [case=dat, number=pl] -> `{root}en`
  [case=gen, number=pl] -> `{root}er`
}

inflection adj_neut for {case, number} {
  requires stems: root
  [case=nom, number=sg] -> `{root}es`
  [case=acc, number=sg] -> `{root}es`
  [case=dat, number=sg] -> `{root}em`
  [case=gen, number=sg] -> `{root}es`
  [case=nom, number=pl] -> `{root}e`
  [case=acc, number=pl] -> `{root}e`
  [case=dat, number=pl] -> `{root}en`
  [case=gen, number=pl] -> `{root}er`
}
```

### verbs.hu

```
@use * from "profile.hu"

# Strong verb with ablaut (a → o in past)
entry faren {
  headword: "faren"
  tags: [pos=verb, register=formal]
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
    note: "Underwent ablaut in the past tense: a -> o."
  }

  examples {
    example {
      tokens: faren[tense=present, person=1, number=sg] "."
      translation: "I go."
    }
  }
}

# Strong verb with ablaut (i → o in past)
entry drinken {
  headword: "drinken"
  tags: [pos=verb]
  stems { pres: "drink", past: "dronk" }
  inflection_class: strong_I
  meaning: "to drink"

  etymology {
    derived_from: faren
    note: "Test derivation link."
  }
}

# Weak (regular) verb
entry maken {
  headword: "maken"
  tags: [pos=verb, register=informal]
  stems { root: "mak" }
  inflection_class: weak
  meaning: "to make, to do"
}

# Irregular "to be" — inline inflection
entry wesen {
  headword: "wesen"
  tags: [pos=verb]
  stems {}
  meaning: "to be"

  inflect for {tense, person, number} {
    [tense=present, person=1, number=sg] -> `bin`
    [tense=present, person=2, number=sg] -> `bist`
    [tense=present, person=3, number=sg] -> `is`
    [tense=present, number=pl, _]        -> `sind`
    [tense=past,    person=1, number=sg] -> `was`
    [tense=past,    person=2, number=sg] -> `warst`
    [tense=past,    person=3, number=sg] -> `was`
    [tense=past,    number=pl, _]        -> `waren`
    [tense=future,  _]                   -> null
  }
}
```

### nouns.hu

```
@use * from "profile.hu"
@reference * as verbs from "verbs.hu"

entry hus {
  headword: "hus"
  tags: [pos=noun, register=formal]
  stems { root: "hus" }
  inflection_class: strong_noun
  meaning: "house, dwelling"

  etymology {
    proto: "*husam"
    note: "Common Germanic root."
  }
}

entry weg {
  headword: "weg"
  tags: [pos=noun]
  stems { root: "weg" }
  inflection_class: strong_noun
  meaning: "way, path, road"

  examples {
    example {
      tokens: "der" weg[case=nom, number=sg]
      translation: "the way"
    }
  }
}
```

### adjectives.hu

```
@use * from "profile.hu"

entry grot {
  headword: "grot"
  tags: [pos=adj]
  stems { root: "grot" }
  inflection_class: adj_strong
  meaning: "great, large"
}

entry alt {
  headword: "alt"
  tags: [pos=adj]
  stems { root: "alt" }
  inflection_class: adj_strong
  meaning: "old"
}
```

### What the Compiler Produces

For `faren` with strong_I and stems `pres: "far", past: "for"`:

| Tense | Person | Number | Form |
|-------|--------|--------|------|
| present | 1 | sg | fare |
| present | 2 | sg | farest |
| present | 3 | sg | fareth |
| present | 1 | pl | faren |
| present | 2 | pl | faren |
| present | 3 | pl | faren |
| past | 1 | sg | for |
| past | 2 | sg | fore |
| past | 3 | sg | for |
| past | 1 | pl | foren |
| past | 2 | pl | foren |
| past | 3 | pl | foren |
| future | 1 | sg | werde faren |
| future | * | * | (null — no form) |

The `werde faren` cell comes from `forms_override`, which overrides the class's `null` for that specific cell.

For `grot` with adj_strong (delegation):

| Case | Number | Gender | Form |
|------|--------|--------|------|
| nom | sg | masc | groter |
| acc | sg | masc | groten |
| nom | sg | fem | grote |
| nom | sg | neut | grotes |
| dat | sg | masc | grotem |
| ... | ... | ... | ... |

24 forms total (4 cases x 2 numbers x 3 genders).

---

## Example 2: An Agglutinative Language

A language where words are built by concatenating morpheme slots. This uses the `compose` syntax.

### profile.hu

```
tagaxis pos {
  role: classificatory
  display: { en: "Part of Speech" }
  index: exact
}

tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

tagaxis person {
  role: inflectional
  display: { en: "Person" }
}

tagaxis number {
  role: inflectional
  display: { en: "Number" }
}

tagaxis negation {
  role: inflectional
  display: { en: "Polarity" }
}

@extend pos_vals for tagaxis pos {
  verb { display: { en: "Verb" } }
  noun { display: { en: "Noun" } }
}

@extend tense_vals for tagaxis tense {
  present { display: { en: "Present" } }
  past    { display: { en: "Past" } }
  future  { display: { en: "Future" } }
}

@extend person_vals for tagaxis person {
  1 { display: { en: "1st" } }
  2 { display: { en: "2nd" } }
  3 { display: { en: "3rd" } }
}

@extend number_vals for tagaxis number {
  sg { display: { en: "Singular" } }
  pl { display: { en: "Plural" } }
}

@extend neg_vals for tagaxis negation {
  pos { display: { en: "Affirmative" } }
  neg { display: { en: "Negative" } }
}

# Agglutinative verb: root + negation + tense + person-number
inflection aggl_verb for {tense, person, number, negation} {
  requires stems: root

  compose root + neg_sfx + tense_sfx + pn_sfx

  slot neg_sfx {
    [negation=pos] -> ``
    [negation=neg] -> `ma`
  }

  slot tense_sfx {
    [tense=present] -> `iyor`
    [tense=past]    -> `di`
    [tense=future]  -> `ecek`
  }

  slot pn_sfx {
    [person=1, number=sg] -> `um`
    [person=2, number=sg] -> `sun`
    [person=3, number=sg] -> ``
    [person=1, number=pl] -> `uz`
    [person=2, number=pl] -> `sunuz`
    [person=3, number=pl] -> `ler`
  }

  # Irregular: negative past 3sg has a special form
  override [negation=neg, tense=past, person=3, number=sg] -> `{root}medi`
}
```

### entries.hu

```
@use * from "profile.hu"

entry gel {
  headword: "gel"
  tags: [pos=verb]
  stems { root: "gel" }
  inflection_class: aggl_verb
  meaning: "to come"

  examples {
    example {
      tokens: gel[tense=present, person=1, number=sg, negation=pos]
      translation: "I am coming"
    }
    example {
      tokens: gel[tense=past, person=3, number=sg, negation=neg]
      translation: "He/she did not come"
    }
  }
}

entry yaz {
  headword: "yaz"
  tags: [pos=verb]
  stems { root: "yaz" }
  inflection_class: aggl_verb
  meaning: "to write"
}
```

### Output for `gel`

Compose evaluation for selected cells:

| Negation | Tense | Person | Number | Composition | Form |
|----------|-------|--------|--------|-------------|------|
| pos | present | 1 | sg | gel + `` + iyor + um | geliyorum |
| pos | past | 3 | sg | gel + `` + di + `` | geldi |
| neg | present | 2 | sg | gel + ma + iyor + sun | gelmayorsun |
| neg | past | 3 | sg | (override) | gelmedi |
| pos | future | 1 | pl | gel + `` + ecek + uz | gelecekuz |

36 forms total (3 tenses x 3 persons x 2 numbers x 2 polarities).

---

## Example 3: Multi-Script Language

A language that uses multiple writing systems, such as a conlang inspired by Japanese:

```
tagaxis pos {
  role: classificatory
  display: { en: "Part of Speech", ja: "品詞" }
  index: exact
}

@extend pos_vals for tagaxis pos {
  verb { display: { en: "Verb", ja: "動詞" } }
  noun { display: { en: "Noun", ja: "名詞" } }
}

entry taberu {
  headword {
    default: "食べる"
    kana: "たべる"
    romaji: "taberu"
  }
  tags: [pos=verb]
  stems {}
  meaning: "to eat"
}

entry neko {
  headword {
    default: "猫"
    kana: "ねこ"
    romaji: "neko"
  }
  tags: [pos=noun]
  stems {}
  meaning: "cat"
}
```

The output stores all scripts in the `headword_scripts` table, with the `default` script used as the primary headword in the `entries` table.

---

## Example 4: Etymology Network

Modeling derivational relationships between entries:

```
@use * from "profile.hu"

entry proto_far {
  headword: "far-"
  tags: [pos=verb]
  stems {}
  meaning: "to go (proto-form)"

  etymology {
    proto: "*per- (PIE)"
    note: "Reconstructed proto-root."
  }
}

entry faren {
  headword: "faren"
  tags: [pos=verb]
  stems { root: "far" }
  meaning: "to go"

  etymology {
    derived_from: proto_far
    proto: "*far-"
    note: "Basic verb, directly from proto-root."
  }
}

entry afaran {
  headword: "afaran"
  tags: [pos=verb]
  stems { root: "afar" }
  meaning: "to depart"

  etymology {
    derived_from: faren
    note: "Prefixed derivative: a- + faren."
  }
}

entry fart {
  headword: "fart"
  tags: [pos=noun]
  stems {}
  meaning: "journey, voyage"

  etymology {
    derived_from: faren
    cognates {
      afaran "shares the same root"
    }
    note: "Deverbal noun from faren."
  }
}
```

This creates a derivation DAG:

```
proto_far → faren → afaran
                  → fart
```

The compiler verifies this is acyclic. If `proto_far` had `derived_from: fart`, it would be a cycle error.

The `links` table will contain:
- `faren → proto_far` (derived_from)
- `afaran → faren` (derived_from)
- `fart → faren` (derived_from)
- `fart → afaran` (cognate)

---

## Example 5: Sharing Definitions Across Files

A larger project that demonstrates the module system:

```
core/
  axes.hu          # shared tag axis declarations
  pos.hu           # part of speech values
  verbal.hu        # verbal category values + inflection classes
  nominal.hu       # nominal category values + declension classes
entries/
  verbs.hu
  nouns.hu
main.hu            # entry point
```

### core/axes.hu

```
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

tagaxis case {
  role: inflectional
  display: { en: "Case" }
}
```

### core/pos.hu

```
@use pos from "../core/axes.hu"

@extend pos_vals for tagaxis pos {
  verb { display: { en: "Verb" } }
  noun { display: { en: "Noun" } }
}
```

### core/verbal.hu

```
@use tense, number from "axes.hu"

@extend tense_vals for tagaxis tense {
  present { display: { en: "Present" } }
  past    { display: { en: "Past" } }
}

@extend number_vals for tagaxis number {
  sg { display: { en: "Singular" } }
  pl { display: { en: "Plural" } }
}

inflection regular_verb for {tense, number} {
  requires stems: root

  [tense=present, number=sg] -> `{root}s`
  [tense=present, number=pl] -> `{root}`
  [tense=past, number=sg]    -> `{root}ed`
  [tense=past, number=pl]    -> `{root}ed`
}
```

### core/nominal.hu

```
@use case, number from "axes.hu"
@use number_vals from "verbal.hu"

@extend case_vals for tagaxis case {
  nom { display: { en: "Nominative" } }
  acc { display: { en: "Accusative" } }
}

inflection regular_noun for {case, number} {
  requires stems: root

  [case=nom, number=sg] -> `{root}`
  [case=acc, number=sg] -> `{root}`
  [case=nom, number=pl] -> `{root}i`
  [case=acc, number=pl] -> `{root}os`
}
```

### entries/verbs.hu

```
@use * from "../core/axes.hu"
@use * from "../core/pos.hu"
@use * from "../core/verbal.hu"

entry walk {
  headword: "walk"
  tags: [pos=verb]
  stems { root: "walk" }
  inflection_class: regular_verb
  meaning: "to move on foot"
}
```

### entries/nouns.hu

```
@use * from "../core/axes.hu"
@use * from "../core/pos.hu"
@use * from "../core/verbal.hu"
@use * from "../core/nominal.hu"
@reference * as verbs from "verbs.hu"

entry town {
  headword: "town"
  tags: [pos=noun]
  stems { root: "town" }
  inflection_class: regular_noun
  meaning: "settlement, town"
}
```

### main.hu

```
@use * from "core/axes.hu"
@use * from "core/pos.hu"
@use * from "core/verbal.hu"
@use * from "core/nominal.hu"
@reference * from "entries/verbs.hu"
@reference * from "entries/nouns.hu"
```

Compile with:

```sh
hubullu main.hu -o my-lang.sqlite
```

---

## Tips and Patterns

### When to Use Named vs. Inline Inflection

- **Named** (`inflection_class`): use for classes with multiple members. Most verbs in a language fall into a handful of conjugation classes.
- **Inline** (`inflect`): use for truly unique words ("to be", "to go" in many languages) that share no pattern with other words.

### Modeling Suppletion

For fully suppletive forms (where the stem is completely different), use inline inflection with literal templates:

```
entry wesen {
  stems {}
  inflect for {tense, person, number} {
    [tense=present, person=1, number=sg] -> `bin`
    [tense=present, person=3, number=sg] -> `is`
    [tense=past, person=1, number=sg]    -> `was`
    ...
  }
}
```

### Modeling Defective Paradigms

Use `null` for forms that don't exist:

```
inflection weather_verb for {tense, person, number} {
  requires stems: root
  # weather verbs only conjugate in 3sg
  [person=3, number=sg, _] -> `{root}...`
  [_]                      -> null
}
```

### Using forms_override for Minor Irregularities

When a word is 95% regular but has one or two irregular forms, use a named class with `forms_override` instead of writing everything inline:

```
entry gehen {
  stems { root: "geh" }
  inflection_class: regular_verb

  forms_override {
    [tense=past, number=sg] -> `ging`
    [tense=past, number=pl] -> `gingen`
  }

  meaning: "to go"
}
```

### Keeping Profiles DRY with Delegation

When your language has complex agreement patterns (e.g., adjectives agreeing in gender), delegate to sub-paradigms rather than writing out every combination. This keeps each paradigm small and readable, and allows reuse.
