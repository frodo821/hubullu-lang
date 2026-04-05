---
name: hut-authoring
description: Assist with writing and editing .hut (hubullu text) files for sentence rendering. Trigger when editing .hut files.
allowed-tools: Read, Grep, Glob, Bash(hubullu render *)
---

You are assisting with `.hut` (hubullu text) file authoring for sentence rendering.
hubullu renders `.hut` files by resolving dictionary entry references against compiled `.hu` databases.

# .hut File Structure

A `.hut` file has these sections:

1. **Comment directives** (optional, before everything else)
2. **@reference directives** (optional, at the beginning)
3. **Token list** (the content to render)

```
# @title My Page Title
@reference * from "main.hu"

"The" cat walk[tense=present, person=3, number=sg] "."
```

Running `hubullu render file.hut` compiles referenced `.hu` files (with caching), resolves tokens, and outputs the rendered text.

# Comment Directives

Lines matching `# @key value` at the top of the file (before any body tokens) are parsed as directives. They must appear before or interleaved with `@reference` lines:

```
# @title My Page Title
```

Currently supported directives:
- `# @title <text>` — sets the page title (used in HTML output for `<title>` and navigation). Overridden by `_config.json` title or CLI `--title` option.

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
gelmek[$=root]                           # Stem extraction (raw stem value)
ns.entry_id                              # Namespaced entry
entry_id#sense                           # Specific meaning sense
ns.entry_id#sense[case=nom, number=sg]   # Full form
```

### Stem Reference `[$=name]`

Extract a raw stem value instead of an inflected form. Useful for composing stem + affix manually:

```
gelmek[$=root]~"iyor"    # → "geliyor" (stem "gel" + "iyor")
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

## XML-like Tags

Tags wrap tokens with markup, passed through to HTML output (stripped in plain text):

```
<em>walk[tense=present, person=3, number=sg]</em>
<strong>"important"</strong>
<a href="http://example.com">"link text"</a>
```

### Attributes

Tags can have `key="value"` attributes:

```
<span class="highlight">cat[number=pl]</span>
```

### Self-closing Tags

```
<br/>
<hr/>
<img src="image.jpg"/>
```

### Custom Elements (Hyphenated Names)

Tag names can contain hyphens for custom elements:

```
<ruby-text>"漢字"</ruby-text>
<my-annotation type="gloss">"word"</my-annotation>
```

### Nesting

Tags can be nested and can wrap multiple tokens:

```
<div class="verse"><em>"In the beginning"</em> "," create[tense=past, person=3, number=sg] "."</div>
```

## Newline Marker `//`

Inserts a line break between adjacent tokens:

```
"first line" // "second line"
# Renders as: "first line\nsecond line"
```

`//` replaces the separator with a newline character. If both `~` and `//` apply, `//` takes priority.

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

# Special Tokens

| Token | Effect |
| ----- | ------ |
| `~` | Suppresses separator (glue adjacent tokens) |
| `//` | Inserts newline instead of separator |
| `<tag>...</tag>` | XML-like tag wrapping child tokens |
| `<tag/>` | Self-closing tag |

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

## XML Tags with Entry References

```
# @title Genesis 1:1
@reference * from "genesis_words.hu"

<em>bara[tense=perfect, person=3, number=sg]</em> "Elohim" <strong>"et"</strong> "ha-shamayim" "."
# HTML → <em>bara</em> Elohim <strong>et</strong> ha-shamayim.
# Text → bara Elohim et ha-shamayim.
```

# Common Errors to Avoid

- Referencing an entry that doesn't exist in any `@reference`'d database
- Using axis names or values not defined in the `.hu` file's tagaxis declarations
- Form spec that matches zero or multiple forms (must be unambiguous)
- Forgetting `@reference` directive (no database to look up entries from)
- Using `~` without adjacent tokens on both sides
- Mismatched XML tags (opening tag without matching closing tag)
- Using non-hyphenated multi-word tag names (use `my-tag`, not `my tag`)
