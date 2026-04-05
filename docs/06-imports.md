# Imports

Hubullu projects typically span multiple files. The import system has two directives — `@use` for declarations and `@reference` for entries — each with distinct scoping and cycle rules.

## @use — Import Declarations

`@use` imports **declarations** (tag axes, `@extend`s, inflection classes, phonrules) from another file:

```
@use * from "profile.hu"
@use tense, number from "core/axes.hu"
@use tense as t, number as n from "core/axes.hu"
```

### What @use Imports

`@use` only brings in:
- `tagaxis` declarations
- `@extend` declarations
- `inflection` declarations
- `phonrule` declarations

Entries in the imported file are **silently ignored** by glob imports (`@use *`). Named imports that target an entry produce a compile error:

```
@use my_entry from "entries.hu"    # ERROR: cannot import entry via @use
```

### Cycle Detection

`@use` imports are resolved recursively using depth-first search. **Circular `@use` chains are a compile error**:

```
# a.hu
@use * from "b.hu"    # ERROR if b.hu also @use's a.hu

# b.hu
@use * from "a.hu"    # circular!
```

## @reference — Import Entries

`@reference` imports **entries** from another file, making them available for references in etymology, examples, and other cross-entry links:

```
@reference * from "entries/verbs.hu"
@reference faren, drinken from "entries/verbs.hu"
```

### What @reference Imports

`@reference` only brings in `entry` definitions. Declarations (tag axes, extends, inflections) are **silently ignored** by glob imports. Named imports targeting a declaration produce a compile error:

```
@reference tense from "profile.hu"    # ERROR: cannot import declaration via @reference
```

### Cycle Tolerance

Unlike `@use`, **`@reference` permits circular references**. This is by design — entry A might reference entry B in its etymology while entry B references entry A:

```
# verbs.hu
@reference * from "nouns.hu"    # OK even if nouns.hu references verbs.hu

# nouns.hu
@reference * as verbs from "verbs.hu"    # circular @reference is allowed
```

The compiler uses a visited-file set to avoid re-loading files, but does not treat cycles as errors.

## @export — Re-export Symbols

`@export` re-exports imported symbols so that files importing from you also receive them:

### Form 1: Re-export Already-Imported Symbols

```
@use * from "core.hu"
@export use *                    # re-export all declarations from core.hu
@export use tense                # re-export only tense
```

### Form 2: Import and Re-export in One Step

```
@export use * from "core.hu"             # import + re-export declarations
@export reference * from "entries.hu"    # import + re-export entries
```

`@export use` handles declarations (tagaxis, @extend, inflection, phonrule).
`@export reference` handles entries.

The syntax for the import target (`*`, `* as ns`, named list) is the same as `@use` and `@reference`.

## Import Syntax

Both `@use` and `@reference` share the same syntax for specifying what to import.

### Glob Import

Import everything (of the appropriate kind):

```
@use * from "profile.hu"
@reference * from "entries/verbs.hu"
```

### Glob with Namespace

Prefix all imported symbols with a namespace:

```
@use * as stdlib from "stdlib/universal.hu"
@reference * as verbs from "entries/verbs.hu"
```

Access namespaced symbols with dot notation:

```
entry foo {
  ...
  etymology {
    cognates {
      verbs.faren "related verb"
    }
  }
}
```

### Named Imports

Import specific items by name:

```
@use tense, number from "core/axes.hu"
@reference faren, drinken from "entries/verbs.hu"
```

### Named Imports with Aliases

Rename items for the local scope:

```
@use tense as t, number as n from "core/axes.hu"
@reference faren as f from "entries/verbs.hu"
```

### Parenthesized Lists

Named imports can optionally be wrapped in parentheses (useful for long lists):

```
@use (
  tense as t,
  number as n,
  person,
  case
) from "core/axes.hu"
```

Trailing commas are allowed before `)` or before `from`.

## Path Resolution

Import paths are resolved **relative to the importing file's directory**:

```
# If this file is at project/entries/verbs.hu:
@use * from "../profile.hu"          # resolves to project/profile.hu
@reference * from "irregular.hu"     # resolves to project/entries/irregular.hu
```

## Standard Library

The `std:` scheme imports built-in modules bundled with the compiler:

```
@use * from "std:ipa"
```

Standard library modules are embedded in the compiler binary — no filesystem path resolution is performed.

## Import Placement

`@use` and `@reference` must appear at the **top of the file**, before any other items. They are **not hoisted** — unlike `tagaxis`, `@extend`, and `inflection`, which can appear in any order within a file.

```
# CORRECT
@use * from "profile.hu"
@reference * from "verbs.hu"

entry foo { ... }

# INCORRECT — imports must come first
entry foo { ... }
@use * from "profile.hu"    # too late
```

## Scope and Visibility

Imported symbols are visible only in the file that imports them. There is no transitive re-export — if `a.hu` imports from `b.hu` and `b.hu` imports from `c.hu`, items from `c.hu` are **not** visible in `a.hu` unless `a.hu` also imports them.

```
# c.hu
tagaxis mood { ... }

# b.hu
@use * from "c.hu"
# mood is visible here

# a.hu
@use * from "b.hu"
# mood is NOT visible here — must also: @use * from "c.hu"
```

## Ambiguity Resolution

If an unqualified name could refer to multiple imported symbols, it's a compile error. Use aliases to disambiguate:

```
# Both files define a 'tense' tagaxis
@use tense as verbal_tense from "verbal.hu"
@use tense as nominal_tense from "nominal.hu"

# Now use the qualified names
inflection verb_class for {verbal_tense, number} { ... }
```

## Summary Table

| Aspect | `@use` | `@reference` |
| ------ | ------ | ------------ |
| **Imports** | tagaxis, @extend, inflection, phonrule | entry |
| **Glob skips** | entries (silent) | declarations (silent) |
| **Named wrong kind** | Compile error | Compile error |
| **Cycles** | Compile error | Allowed |
| **Placement** | File top only | File top only |
| **Hoisted** | No | No |

`@export` re-exports symbols transitively. Without `@export`, imported symbols are visible only in the importing file.

### Standard Library

| Path scheme | Resolution |
| ----------- | ---------- |
| `"profile.hu"` | Relative to importing file |
| `"../core.hu"` | Relative path |
| `"std:ipa"` | Built-in module (compiler-embedded) |

## Recommended Project Layout

```
my-lang/
  profile.hu              # tag axes, extends, inflection classes
  main.hu                 # entry point — @use profile, @reference entry files
  entries/
    verbs.hu              # @use profile, defines verb entries
    nouns.hu              # @use profile, @reference verbs (for etymology)
    adjectives.hu         # @use profile
  stdlib/                  # optional shared definitions
    universal_pos.hu
```

The entry point `main.hu` typically does `@use * from "profile.hu"` for declarations and `@reference * from "entries/verbs.hu"` (etc.) for entry files that need cross-referencing.

Each entry file does its own `@use * from "profile.hu"` to bring in the declarations it needs. Entry files that need to reference entries from other files use `@reference`.
