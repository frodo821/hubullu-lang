---
name: hut-authoring
description: Assist with writing and editing .hut (hubullu text) files for sentence rendering. Trigger when editing .hut files.
allowed-tools: Read, Grep, Glob, Bash(hubullu render *)
---

You are assisting with `.hut` (hubullu text) file authoring for sentence rendering.
hubullu renders `.hut` files by resolving dictionary entry references against compiled `.hu` databases.

# .hut File Structure

A `.hut` file has two sections:

1. **@reference directives** (optional, at the beginning)
2. **Token list** (the content to render)

```
@reference * from "main.hu"

"The" cat walk[tense=present, person=3, number=sg] "."
```

Running `hubullu render file.hut` compiles referenced `.hu` files (with caching), resolves tokens, and outputs the rendered text.

# @reference Directives

Declare which `.hu` databases to use for entry lookup:

```
@reference * from "path/to/file.hu"             # All entries, unnamespaced
@reference * as ns from "path/to/file.hu"       # All entries under namespace (ns.entry_id)
@reference entry1, entry2 from "path/to/file.hu" # Named imports only
@reference entry1, entry2 as alias from "path/to/file.hu"  # With alias
```

Paths are resolved relative to the `.hut` file's directory.

# Token Types

## String Literals

Plain text, rendered as-is:

```
"The" "." "," "«" "»" "and"
```

## Entry References

Look up dictionary entries and their inflected forms:

```
cat                                      # Headword (dictionary form)
cat[]                                    # Same as above
cat[number=pl]                           # Specific inflected form
walk[tense=present, person=3, number=sg] # Multiple tag conditions
ns.entry_id                              # Namespaced entry
entry_id#sense                           # Specific meaning sense
ns.entry_id#sense[case=nom, number=sg]   # Full form
```

### Form Specification `[axis=value, ...]`

- All specified conditions must match exactly
- Order of conditions doesn't matter
- Common axes: `tense`, `person`, `number`, `case`, `gender`, `mood`, `voice`, `negation`
- Values depend on the tagaxis definitions in the `.hu` file
- Omitting `[]` or using `[]` empty returns the headword

### Resolution

1. Entry is looked up by name in referenced databases (unnamespaced sources searched in declaration order)
2. Without form spec → headword is returned
3. With form spec → the inflected form matching all tag conditions is returned

## Glue Marker `~`

Suppresses the separator between adjacent tokens. Used for clitics, compound words, etc.:

```
dicere[tense=perfect, person=3, number=sg]~"que"
# Renders as: "dixitque" (no space before "que")
```

```
"mal"~"bon"~"a" "hundo"
# Renders as: "malbona hundo"
```

# Comments

`#` at line start or after whitespace is a line comment:

```
# This is a comment
@reference * from "main.hu"

# Sentence 1
"The" cat walk[tense=present, person=3, number=sg] "."
```

# Rendering Rules

## Separator

Tokens are joined with a separator (default: `" "` space).

## Punctuation

Separator is suppressed before certain characters (default: `.,;:!?`).

```
"The" cat walk[tense=present, person=3, number=sg] "."
# → "The cat walks."  (no space before period)
```

## Glue

`~` suppresses the separator on both sides:

```
"a"~"b" "c"
# → "ab c"
```

## Configuration

Separator and punctuation rules are configured via `@render` in the `.hu` file:

```
@render {
  separator: " "
  no_separator_before: ".,;:!?"
}
```

# Examples

## Simple English

```
@reference * from "main.hu"

"The" cat walk[tense=present, person=3, number=sg] "."
# → The cat walks.
```

## Latin with Enclitics

```
@reference * from "genesis_words.hu"

puella[case=nom, number=sg] servus[case=acc, number=sg] amare[tense=present, person=3, number=sg, voice=active] "."
# → puella servum amat.

dicere[tense=perfect, person=3, number=sg]~"que" Deus "."
# → dixitque Deus.
```

## Multiple Databases

```
@reference * as en from "english.hu"
@reference * as la from "latin.hu"

en.cat[number=pl] "=" la.feles[case=nom, number=pl]
# → cats = feles
```

# Common Errors to Avoid

- Referencing an entry that doesn't exist in any `@reference`'d database
- Using axis names or values not defined in the `.hu` file's tagaxis declarations
- Form spec that matches zero or multiple forms (must be unambiguous)
- Forgetting `@reference` directive (no database to look up entries from)
- Using `~` without adjacent tokens on both sides
