# Language Basics

## File Structure

A LexDSL file consists of top-level items in any order (with the exception that `@use` and `@reference` must precede other items). The eight top-level items are:

| Item | Purpose |
| ---- | ------- |
| `@use` | Import declarations (tag axes, extends, inflections, phonrules) from another file |
| `@reference` | Import entries from another file |
| `tagaxis` | Declare a grammatical or classificatory dimension |
| `@extend` | Add enumerated values to an existing tag axis |
| `inflection` | Define a paradigm (inflection/declension pattern) |
| `phonrule` | Define phonological rewrite rules (e.g. vowel harmony) |
| `@render` | Configure token rendering (separator, punctuation handling) |
| `entry` | Define a dictionary entry |

A minimal file might contain just a single entry, or just tag axis definitions — there is no required structure beyond the rule that imports come first.

## Comments

Comments begin with `#` and extend to the end of the line:

```
# This is a comment
tagaxis tense {    # This is also a comment
  role: inflectional
}
```

**Important**: `#` is context-sensitive. It is a comment only when preceded by whitespace (or at the start of a line). After a non-whitespace character, `#` is the **meaning separator** token used in entry references:

```
faren#motion       # '#' here is the meaning separator, not a comment
                   # '#' here IS a comment (preceded by whitespace)
```

## Identifiers

Identifiers follow Unicode rules (UAX #31):

- Can start with any Unicode `XID_Start` character (letters from any script) or `_` followed by `XID_Continue`
- Can contain `XID_Continue` characters (letters, digits, combining marks, `_`)
- **Digits are valid identifier starts** — this allows numeric tag values like `person=1`

Valid identifiers:

```
tense              # ASCII
parts_of_speech    # underscores
品詞               # CJK characters
たべる             # Hiragana
1                  # digit (for tag values like person=1)
_internal          # leading underscore
```

## The Underscore Token

A standalone `_` (not followed by an identifier character) is the **wildcard token**, used in tag condition lists to mean "match anything for unspecified axes":

```
[tense=present, _]     # _ is the wildcard — match any person, number, etc.
```

But `_foo` is an identifier (the `_` is part of the name).

## String Literals

Double-quoted strings are plain text with no interpolation:

```
"to go, to travel"
"house"
"食べる"
```

Escape sequences: `\\`, `\"`, `\n`, `\t`.

Strings are used for headwords, meanings, translations, notes, display text, and other free-form text.

## Template Literals

Backtick-delimited templates support stem interpolation:

```
`{root}ed`          # inserts the value of the "root" stem
`{pres}en`          # inserts the value of the "pres" stem
`{root1.C1}a{root1.C2}a{root1.C3}a`   # structural slot interpolation
``                  # empty template (produces empty string)
```

Templates can **only** appear in inflection rule right-hand sides and `forms_override`. Using a template where a string is expected (or vice versa) is a compile error.

Escape sequences: `` \` ``, `\{`, `\}`, `\\`.

See [Templates](07-templates.md) for the full template reference.

## Hoisting

Declarations within the same file can reference each other regardless of definition order. The following items are **hoisted** (forward-referenceable):

| Item | Hoisted? |
| ---- | -------- |
| `tagaxis` | Yes |
| `@extend` | Yes |
| `inflection` | Yes |
| `phonrule` | Yes |
| `entry` | No |
| `@use` / `@reference` | No (must appear at file top) |

This means you can write an inflection class that references a tag axis defined later in the same file:

```
# This works — strong_I references tense/number before they're defined
inflection strong_I for {tense, number} {
  requires stems: pres, past
  [tense=present, number=sg] -> `{pres}s`
  [tense=present, number=pl] -> `{pres}`
  [tense=past, number=sg]    -> `{past}`
  [tense=past, number=pl]    -> `{past}en`
}

tagaxis tense {
  role: inflectional
  display: { en: "Tense" }
}

tagaxis number {
  role: inflectional
  display: { en: "Number" }
}
```

## The Arrow Token

`->` is a single token (not two characters) used in inflection rules to separate the condition from the result:

```
[tense=present, number=sg] -> `{root}s`
```

A bare `-` is not valid and produces a compile error.

## Punctuation Summary

| Token | Meaning |
| ----- | ------- |
| `{ }` | Block delimiters |
| `[ ]` | Tag condition lists, form specs |
| `( )` | Grouped import lists |
| `:` | Field separator |
| `,` | List separator |
| `.` | Namespace qualifier |
| `#` | Meaning separator (after non-whitespace) or comment (after whitespace) |
| `=` | Tag assignment (`axis=value`) |
| `->` | Rule arrow |
| `+` | Compose concatenation |
| `_` | Wildcard in tag conditions |
| `*` | Glob import |
| `~` | Glue marker (suppresses separator between tokens in examples) |
| `\|` | Union operator (in `phonrule` character class definitions) |
| `!` | Negation (in `phonrule` context patterns) |
| `/` | Context separator (in `phonrule` rewrite rules) |
