---
name: hu-authoring
description: Assist with writing and editing .hu (hubullu) dictionary files. Trigger when editing .hu files or asking about hubullu syntax.
allowed-tools: Read, Grep, Glob, Bash(hubullu compile *)
---

You are assisting with `.hu` file authoring for the hubullu compiler.
hubullu compiles `.hu` dictionary definition files into SQLite databases.

# .hu File Structure

A `.hu` file contains these top-level constructs (order-independent except imports):

```
@use ... from "path.hu"           # Import declarations (must be at top)
@reference ... from "path.hu"     # Import entries (must be at top)
@export use ...                   # Re-export declarations (must be at top)
@export reference ...             # Re-export entries (must be at top)
@render { ... }                   # Rendering config (optional)
tagaxis NAME { ... }              # Grammatical axis definition
@extend NAME for tagaxis AXIS { ... }  # Add values to an axis
inflection NAME for {AXES} { ... }     # Inflection paradigm
phonrule NAME { ... }                  # Phonological rules
entry NAME { ... }                     # Dictionary entry
```

## Comments

`#` at line start or after whitespace is a line comment. `#` after non-whitespace is a meaning separator token (e.g., `entry#sense`).

## Identifiers

Letters, digits, and underscores are valid in identifiers. Digits alone are also valid identifiers (e.g., `person=1`).

## String Types

- `"..."` — plain string literal, no interpolation
- `` `...` `` — template literal with `{stem}` or `{stem.slot}` interpolation (used in inflection rules)

# Imports

## @use (Declarations)

Imports tagaxis, @extend, and inflection from another file. Does NOT import entries.

```
@use * from "core/tags.hu"              # All declarations
@use * as stdlib from "core/tags.hu"    # Namespaced (stdlib.tense)
@use tense, aspect from "core/tags.hu"  # Named imports
```

### Standard Library (`std:` scheme)

Built-in modules can be imported via the `std:` URL scheme. No file I/O — modules are embedded in the compiler.

```
@use * from "std:_test"                  # Import built-in test module
@use * as stdlib from "std:module_name"  # Namespaced std import
```

Circular `@use` is a compile error.

## @reference (Entries)

Imports entries from another file. Does NOT import declarations.

```
@reference * from "entries/verbs.hu"
@reference * as verbs from "entries/verbs.hu"   # verbs.faren
@reference faren, maken from "entries/verbs.hu" # Named
```

Circular `@reference` is allowed (entries are resolved in phase 2).

## @export (Re-exports)

Re-exports symbols transitively so downstream files receive them too.

```
@export use * from "core/tags.hu"         # Re-export all declarations from file
@export use tense from "core/tags.hu"     # Re-export named declarations from file
@export use *                             # Re-export everything this file @use'd
@export reference * from "entries.hu"     # Re-export entries from file
@export reference *                       # Re-export everything this file @reference'd
```

Two forms:
- **Form 1** (`@export use/reference <target>`) — re-exports what this file already imported
- **Form 2** (`@export use/reference <target> from "file"`) — imports and re-exports in one step

# tagaxis

Define a grammatical dimension:

```
tagaxis tense {
  role: inflectional    # inflectional | classificatory | structural
  display: { en: "Tense", ja: "時制" }  # Optional, multi-language
  index: exact          # exact | fulltext | omit for no index
}
```

# @extend

Add values to a tagaxis (only effective in the importing file's scope):

```
@extend pos_values for tagaxis parts_of_speech {
  verb {
    display: { en: "Verb", ja: "動詞" }
  }
  noun {
    display: { en: "Noun" }
  }
}
```

For structural axes, define slots:

```
@extend root_templates for tagaxis root_type {
  triliteral {
    slots: [C1, C2, C3]
  }
}
```

# inflection

Define inflection paradigms with tag conditions mapping to forms:

```
inflection strong_verb for {tense, person, number} {
  display: { en: "Strong Verb", ja: "強変化動詞" }  # Optional display name
  requires stems: pres, past

  [tense=present, person=1, number=sg] -> `{pres}e`
  [tense=present, person=3, number=sg] -> `{pres}t`
  [tense=present, number=pl, _]        -> `{pres}en`
  [tense=past, _]                      -> `{past}`
}
```

## Tag Conditions

- `[axis=value, axis=value]` — exact match
- `[axis=value, _]` — `_` matches remaining axes (must be last)
- `[_]` — matches anything

All tag combinations must be covered (including via `null` for nonexistent forms).

## null Forms

```
[tense=future, _] -> null    # This form does not exist
```

## Stem Constraints

For structural stems with slots:

```
requires stems: root[root_type=triliteral]

[person=3, number=sg] -> `{root.C1}a{root.C2}a{root.C3}a`
```

## Phonological Rule Application in Rules

Apply a phonrule directly to a template in the RHS:

```
[case=nom, number=sg] -> harmony(`{root}ler`)
```

## Delegation

Dispatch to another inflection class:

```
[gender=masc, _] -> adj_masc[case, number] with stems { nom: root }
```

- `case` (bare) = pass through the caller's value
- `case=nominative` = pass fixed value
- `with stems { ... }` maps the delegate's required stems to the caller's stems

## compose (Agglutinative Morphology)

Build forms by concatenating slots with phonological rules:

```
inflection verb_conj for {tense, person, number, negation} {
  requires stems: root

  compose harmony(elision(root + neg_sfx + tense_sfx + pn_sfx))

  slot neg_sfx {
    [negation=pos, _] -> ``
    [negation=neg, _] -> `mi`
  }

  slot tense_sfx {
    [tense=present, _] -> `iyor`
    [tense=past, _]    -> `di`
  }

  slot pn_sfx {
    [person=1, number=sg] -> `um`
    [person=2, number=sg] -> `sun`
  }

  override [special_case=true] -> `irregular`
}
```

# phonrule

Phonological rewrite rules:

```
phonrule harmony {
  class front = ["e", "i", "ö", "ü"]
  class back  = ["a", "ı", "o", "u"]
  class V = front | back

  map to_back = c -> match {
    "e" -> "a",
    "ö" -> "o",
    else -> c
  }

  V -> to_back / back !V* + !V* _
}
```

- `class NAME = [...]` or union `A | B`
- `map NAME = param -> match { ... }`
- `PATTERN -> REPLACEMENT / LEFT _ RIGHT` — context-sensitive rewrite
- Context elements: `+` (morpheme boundary), `^` (word start), `$` (word end), `(a | b)` (alternation)
- Empty pattern for insertion: `"" -> "e" / C + _ C` (epenthesis)

```
# Devoicing: before consonant or word end
"b" -> "p" / _ (C | $)

# Voicing: at word start or after vowel
"k" -> "g" / (^ | V) _
```

# entry

Dictionary entries:

```
entry faren {
  headword: "faren"
  # Or multi-script: headword { default: "faren", kana: "ファーレン" }

  tags: [pos=verb, register=formal]
  stems { pres: "far", past: "for" }

  inflection_class: strong_verb
  # Or inline: inflect for {tense, person} { ... }

  meaning: "to go"
  # Or multiple: meanings { motion { "to go" } progress { "to advance" } }

  # Optional:
  forms_override {
    [tense=past, person=1, number=sg] -> `ging`
  }

  etymology {
    proto: "*far-"
    cognates { afaran "prefixed" }
    derived_from: source_entry
    note: "PIE root"
  }

  examples {
    example {
      tokens: faren[tense=present, person=1, number=sg] "."
      translation: "I go."
    }
  }
}
```

## Entry References

Used in etymology and examples:

```
entry_id                              # Entry only
entry_id#sense                        # Specific meaning
entry_id[tense=present, number=sg]    # Specific form
entry_id[$=root]                      # Stem extraction
namespace.entry_id#sense[form_spec]   # Full form
```

### Stem References `[$=name]`

Extract a raw stem value instead of an inflected form. Useful for manually composing stem + affix:

```
tokens: gelmek[$=root]~"iyor"    # → "geliyor" (gel + iyor)
```

# @render

Configure text rendering for .hut files:

```
@render {
  separator: " "
  no_separator_before: ".,;:!?"
}
```

# Hoisting Rules

| Construct      | Hoisted | Forward-reference OK |
|----------------|---------|---------------------|
| tagaxis        | Yes     | Yes                 |
| @extend        | Yes     | Yes                 |
| inflection     | Yes     | Yes                 |
| entry          | No      | No                  |
| @use/@reference| No      | Must be at top      |

# Common Errors to Avoid

- Circular `@use` imports (circular `@reference` is fine)
- `_` not at the end of a condition list
- Incomplete paradigm coverage (all tag combinations must be handled, use `null` for nonexistent forms)
- Ambiguous rules (multiple rules with same specificity matching the same conditions)
- Referencing undefined stems or slots in templates
- Using `@use` to import entries or `@reference` to import declarations
- Redefining existing values in `@extend`
