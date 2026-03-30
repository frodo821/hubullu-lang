# Phonological Rules

Phonological rules (`phonrule`) define sound changes that apply to generated word forms. They are essential for modeling vowel harmony, consonant assimilation, and other phonological processes common in agglutinative and fusional languages.

## Basic Structure

```
phonrule <name> {
  class <name> = [<string>, <string>, ...]
  class <name> = <class> | <class>

  map <name> = <param> -> match {
    <string> -> <string>,
    <string> -> <string>,
    else -> <param>
  }

  <pattern> -> <replacement> / <left_context> _ <right_context>
}
```

## Character Classes

Character classes define sets of characters for pattern matching:

### Literal Lists

```
class front = ["e", "i", "ö", "ü"]
class back  = ["a", "o", "u"]
```

### Union of Classes

```
class vowel = front | back
```

A union class matches any character in any of the constituent classes.

## Maps

Maps define character-to-character transformations:

```
map to_back = c -> match {
  "e" -> "a",
  "i" -> "u",
  "ö" -> "o",
  "ü" -> "u",
  else -> c
}
```

- **`<param>`**: a variable name bound to the matched character
- **`match { }`**: pattern-matching arms mapping input to output
- **`else -> <param>`**: fallback that returns the input unchanged (or a literal)

The `else` arm can reference the parameter variable or produce a literal string.

## Rewrite Rules

Rewrite rules describe sound changes with optional context:

```
<from_pattern> -> <replacement>
<from_pattern> -> <replacement> / <left_context> _ <right_context>
```

### From Pattern

What to match:

- **Class reference**: `vowel` — matches any character in the class
- **Literal string**: `"k"` — matches the literal character

### Replacement

What to replace with:

- **Map reference**: `to_back` — applies the named map to the matched character
- **Literal string**: `"k"` — replaces with the literal
- **`null`** — deletes the matched character

### Context

Context restricts where the rule applies. The `_` marks the position of the matched character:

```
vowel -> to_back / back !back* + !back* _
```

This reads: "apply `to_back` to any vowel that is preceded (across morpheme boundaries) by a back vowel with no intervening back vowels."

#### Context Elements

| Element | Meaning |
| ------- | ------- |
| `class` | Matches a character in the class |
| `!class` | Matches a character NOT in the class |
| `"lit"` | Matches a literal string |
| `+` | Matches a morpheme boundary (also matches word start/end) |
| `^` | Matches word start only |
| `$` | Matches word end only |
| `class*` | Matches zero or more characters in the class |
| `!class*` | Matches zero or more characters NOT in the class |
| `class+` | Matches one or more characters in the class (use `!` prefix for negation) |
| `(a \| b)` | Matches if any alternative matches (alternation) |

Context elements can appear on either side of `_`. Left context reads right-to-left from the match position; right context reads left-to-right.

`^` is used in left context, `$` is used in right context. `+` matches morpheme boundaries (`\0`) as well as word edges, while `^` and `$` match only the true start/end of the word.

#### Alternation

Use `(... | ...)` to match any of several alternatives:

```
# Devoice b before a consonant or at word end
"b" -> "p" / _ (C | $)

# Voice k at word start or after a vowel
"k" -> "g" / (^ | V) _
```

Alternation can be combined with `*`: `(C | V)*` matches zero or more characters that are either consonants or vowels.

### Application Order

- All matching positions are found first, then all replacements are applied simultaneously
- Rules are applied iteratively until no more changes occur (convergence)
- Morpheme boundaries (from `compose` `+` operators) are represented internally and are visible to context patterns

## Complete Example: Vowel Harmony

```
phonrule harmony {
  class back  = ["a", "ı", "o", "u"]
  class front = ["e", "i", "ö", "ü"]
  class vowel = back | front

  map to_back = c -> match {
    "e" -> "a",
    "i" -> "ı",
    "ö" -> "o",
    "ü" -> "u",
    else -> c
  }

  map to_front = c -> match {
    "a" -> "e",
    "ı" -> "i",
    "o" -> "ö",
    "u" -> "ü",
    else -> c
  }

  vowel -> to_back  / back  !vowel* + !vowel* _
  vowel -> to_front / front !vowel* + !vowel* _
}
```

This defines a vowel harmony system where suffix vowels assimilate to the frontness/backness of the preceding stem vowel, propagating across morpheme boundaries.

### Using the Phonrule

In inflection rules:

```
[tense=present, _] -> harmony(`{root}ler`)
```

In compose chains:

```
compose harmony(root + tense_sfx + pn_sfx)
```

## Where Phonrules Can Be Used

1. **Inflection rule RHS**: `[conditions] -> harmony(\`{root}ler\`)`
2. **Compose chain**: `compose harmony(root + sfx1 + sfx2)`

Phonrules are imported via `@use` like other declarations. They are hoisted within the same file.
