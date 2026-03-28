# Templates

Templates are backtick-delimited strings with interpolation — the mechanism by which inflection rules produce word forms from stems.

## Syntax

Templates are delimited by backticks:

```
`{root}ed`
`{pres}en`
``              # empty template (produces empty string)
```

### Escape Sequences

| Sequence | Produces |
| -------- | -------- |
| `` \` `` | literal backtick |
| `\{` | literal `{` |
| `\}` | literal `}` |
| `\\` | literal backslash |

## Stem Interpolation

`{name}` inserts the value of a named stem:

```
inflection weak for {tense, number} {
  requires stems: root

  [tense=present, number=sg] -> `{root}s`
  [tense=present, number=pl] -> `{root}`
  [tense=past, number=sg]    -> `{root}ed`
  [tense=past, number=pl]    -> `{root}ed`
}
```

For an entry with `stems { root: "walk" }`:
- `{root}s` → `walks`
- `{root}ed` → `walked`

### Multiple Stems

Templates can reference any stem declared in `requires stems`:

```
inflection strong_I for {tense, number} {
  requires stems: pres, past

  [tense=present, number=sg] -> `{pres}s`
  [tense=past, number=sg]    -> `{past}`
}
```

For `stems { pres: "far", past: "for" }`:
- `{pres}s` → `fars`
- `{past}` → `for`

### Undefined Stems

If a template references a stem name not in the entry's stems, the compiler reports an "undefined stem" error.

## Structural Slot Interpolation

For languages with non-concatenative morphology (like Semitic root-and-pattern systems), templates can reference individual **slots** within a structural stem:

```
{stem.slot}
```

### Setup

First, declare a structural axis with slots:

```
tagaxis stem_type {
  role: structural
  display: { en: "Stem Type" }
}

@extend stem_types for tagaxis stem_type {
  consonantal_3 {
    display: { en: "Triconsonantal Root" }
    slots: [C1, C2, C3]
  }
}
```

Then declare a stem with a constraint:

```
inflection form_I for {tense, person, number} {
  requires stems: root[stem_type=consonantal_3], aux

  [tense=perfect, person=3, number=sg] -> `{root.C1}a{root.C2}a{root.C3}a`
  [tense=imperfect, person=3, number=sg] -> `ya{root.C1}{root.C2}u{root.C3}u`
  [tense=present, _] -> `{aux}u`
  [_] -> null
}
```

### How It Works

1. The `stem_type` axis is `structural`, so its values can have `slots`
2. `consonantal_3` declares three slots: `C1`, `C2`, `C3`
3. `requires stems: root[stem_type=consonantal_3]` constrains the `root` stem to this type
4. In templates, `{root.C1}` extracts slot `C1` from the `root` stem

For an entry with a triconsonantal root `k-t-b`:
- `{root.C1}a{root.C2}a{root.C3}a` → `kataba` (perfect 3sg)
- `ya{root.C1}{root.C2}u{root.C3}u` → `yaktubu` (imperfect 3sg)

### Error Cases

- `{root.C4}` when `consonantal_3` only has `[C1, C2, C3]` → "undefined slot" error
- `{nostem.C1}` when `nostem` is not declared → "undefined structural stem" error

## Where Templates Can Appear

Templates are valid **only** in these contexts:

1. **Inflection rule right-hand sides**:
   ```
   [tense=present, number=sg] -> `{root}s`
   ```

2. **Slot rule right-hand sides** (inside `compose`):
   ```
   slot tense_sfx {
     [tense=past] -> `ta`
   }
   ```

3. **`forms_override` right-hand sides**:
   ```
   forms_override {
     [tense=future, person=1, number=sg] -> `werde faren`
   }
   ```

4. **`override` right-hand sides** (inside `compose`):
   ```
   override [tense=past, person=3, number=sg] -> `{root}tta`
   ```

Using a template where a plain string is expected (e.g., in `meaning`, `note`, `headword`) is a type error. Conversely, using a plain string `"..."` where a template is expected is also an error.

## Templates in Compose

In a `compose` body, each `slot` contains rules with template right-hand sides. The slot templates are evaluated independently and then concatenated:

```
compose root + tense_sfx + pn_sfx

slot tense_sfx {
  [tense=present] -> ``       # empty template
  [tense=past]    -> `ta`
}

slot pn_sfx {
  [person=1, number=sg] -> `m`
  [person=2, number=sg] -> `n`
}
```

For `root = "gel"`, tense=past, person=1, number=sg:
- `root` → `gel` (stem lookup)
- `tense_sfx` → `ta`
- `pn_sfx` → `m`
- Result: `geltam`

## Empty Templates

An empty template (` `` `) produces an empty string. This is useful for:

- A slot that contributes nothing in certain forms:
  ```
  [tense=present] -> ``    # no tense suffix in present
  ```

- A form that is just the bare stem (template with only interpolation):
  ```
  [case=nom, number=sg] -> `{root}`
  ```

## Literal Text in Templates

Any text outside `{ }` in a template is literal:

```
`{root}eth`        # "eth" is literal
`werde {root}en`   # "werde " and "en" are literal
`un{root}lich`     # prefix and suffix are literal
```
