# Inflection

Inflection definitions are the heart of Hubullu. They describe how words change form across grammatical dimensions — conjugation tables for verbs, declension tables for nouns, and any other morphological paradigm.

## Basic Structure

```
inflection <name> display { <lang>: "<text>", ... } for {<axis>, <axis>, ...} {
  requires stems: <stem>, <stem>, ...

  [<conditions>] -> <result>
  [<conditions>] -> <result>
  ...
}
```

- **`<name>`**: unique identifier for this paradigm
- **`display { }`**: optional multilingual display names for the inflection class (stored in `inflection_display` table)
- **`for { }`**: declares which inflectional axes define the paradigm space
- **`requires stems`**: stems that entries must provide to use this class
- **Rules**: map tag conditions to word forms

## The Paradigm Space

The `for { }` clause declares the axes. The compiler computes the **cartesian product** of all values on those axes — every combination must be accounted for by exactly one rule (or explicitly marked `null`).

For example, with `for {tense, number}` where tense has `present, past` and number has `sg, pl`, the paradigm space is:

| | sg | pl |
|---|---|---|
| **present** | ? | ? |
| **past** | ? | ? |

That's 4 cells. Each must be covered.

## Rules

A rule maps a tag condition to a result:

```
[tense=present, number=sg] -> `{root}s`
```

### Tag Condition Lists

Tag conditions are comma-separated `axis=value` pairs inside `[ ]`:

```
[tense=present, person=1, number=sg]    # exact match for all 3 axes
[tense=present, number=pl, _]           # match any person
[tense=future, _]                       # match any person and number
[_]                                     # match everything (catch-all)
```

**The wildcard `_`** means "all unspecified axes match any value." It must appear at the **end** of the condition list. A rule without `_` must specify every axis in `for { }`.

### Rule Right-Hand Sides

A rule's right-hand side can be:

1. **A template literal** — produces a word form:
   ```
   [tense=present, number=sg] -> `{root}s`
   ```

2. **`null`** — the form intentionally does not exist:
   ```
   [tense=future, _] -> null
   ```

3. **A delegation** — delegates to another inflection class (see [Delegation](#delegation) below)

4. **A phonrule application** — applies a phonological rule to a template (see [Phonological Rules](08-phonrules.md)):
   ```
   [tense=present, person=1, number=sg] -> harmony(`{root}ler`)
   ```

### Specificity and Matching

When multiple rules could match a cell, the **most specific** rule wins. Specificity = number of explicit conditions (not counting `_`):

```
[tense=present, person=1, number=sg]   # specificity 3
[tense=present, number=pl, _]          # specificity 2
[tense=future, _]                      # specificity 1
[_]                                    # specificity 0
```

Rules:
- Higher specificity always wins
- If two rules match with **equal specificity**, it's a compile error (ambiguous)
- If no rule matches a cell, it's a compile error (incomplete paradigm)

### Apply (Paradigm-Wide Phonrule)

You can apply a phonological rule to **every non-delegate cell** in the paradigm by adding an `apply` expression before the rules:

```
inflection harmonic_verb for {tense, number} {
  requires stems: root
  apply harmony(cell)

  [tense=present, number=sg] -> `{root}ler`
  [tense=past, number=sg]    -> `{root}di`
  [_]                        -> null
}
```

`cell` is a terminal representing the evaluated rule result. Phonrule applications nest: `apply harmony(elision(cell))` applies `elision` first, then `harmony`.

Delegate results (rules that forward to another inflection) are **not** affected by `apply`.

### Example: A Complete Verb Conjugation

```
inflection strong_I for {tense, person, number} {
  requires stems: pres, past

  # Present tense — specific singular forms, one plural rule
  [tense=present, person=1, number=sg] -> `{pres}e`
  [tense=present, person=2, number=sg] -> `{pres}est`
  [tense=present, person=3, number=sg] -> `{pres}eth`
  [tense=present, number=pl, _]        -> `{pres}en`

  # Past tense
  [tense=past, person=1, number=sg]    -> `{past}`
  [tense=past, person=2, number=sg]    -> `{past}e`
  [tense=past, person=3, number=sg]    -> `{past}`
  [tense=past, number=pl, _]          -> `{past}en`

  # No future tense in this language
  [tense=future, _]                    -> null
}
```

This covers all 18 cells (3 tenses x 3 persons x 2 numbers). The `null` rules account for the 6 future-tense cells.

## Stems

### `requires stems`

Declares the stems that entries must provide to use this inflection class:

```
inflection weak for {tense, person, number} {
  requires stems: root
  ...
}
```

An entry using this class must have:

```
entry maken {
  stems { root: "mak" }
  inflection_class: weak
  ...
}
```

### Multiple Stems

Languages with ablaut, vowel harmony, or other stem alternations can require multiple stems:

```
inflection strong_I for {tense, person, number} {
  requires stems: pres, past

  [tense=present, _] -> `{pres}...`
  [tense=past, _]    -> `{past}...`
}
```

Entry:

```
entry faren {
  stems { pres: "far", past: "for" }
  inflection_class: strong_I
  ...
}
```

### Stem Constraints

Stems can have constraints that restrict which structural type they accept:

```
requires stems: root[stem_type=consonantal_3], aux
```

This declares that `root` must be a stem of structural type `consonantal_3` (which provides slots `C1`, `C2`, `C3`), while `aux` is an unconstrained stem.

## The `null` Value

`null` explicitly marks a form as **intentionally nonexistent** — not a mistake, not a gap, but a deliberate statement that this form does not exist in the language.

```
[tense=future, _] -> null    # this language has no future tense
```

This is distinct from a missing rule, which is a compile error. Using `null`:
- Satisfies the paradigm completeness check
- Produces no entry in the output `forms` table
- Documents that the gap is intentional

A catch-all `null` is useful for partially-filled paradigms:

```
[_] -> null    # everything not covered above doesn't exist
```

## Compose (Agglutinative Paradigms)

For agglutinative languages where word forms are built by concatenating morpheme slots, use the `compose` syntax:

```
inflection regular_verb for {tense, person_number} {
  requires stems: root

  compose root + tense_sfx + pn_sfx

  slot tense_sfx {
    [tense=present] -> ``
    [tense=past]    -> `ta`
    [tense=future]  -> `ru`
  }

  slot pn_sfx {
    [person=1, number=sg] -> `m`
    [person=2, number=sg] -> `n`
    [person=3, number=sg] -> `s`
    [number=pl, _]        -> `ri`
  }

  override [tense=past, person=3, number=sg] -> `{root}tta`
}
```

### How Compose Works

1. **`compose <slot> + <slot> + ...`**: declares the concatenation order. Each element is either a stem name (looked up from the entry) or a named slot defined below.

2. **`slot <name> { rules }`**: defines a morpheme slot with its own set of rules. Each slot is independently matched against the current cell.

3. For each cell in the paradigm:
   - Check `override` rules first — if one matches, use it directly
   - Otherwise, concatenate: evaluate each slot's rules for the current cell and join the results
   - If any slot evaluates to `null`, the entire form is `null`

### Compose with Phonological Rules

The compose chain can be wrapped in a phonological rule to apply sound changes across morpheme boundaries:

```
inflection turkish_verb for {tense, person, number} {
  requires stems: root

  compose harmony(root + tense_sfx + pn_sfx)

  slot tense_sfx {
    [tense=present] -> `iyor`
    [tense=past]    -> `di`
  }

  slot pn_sfx {
    [person=1, number=sg] -> `um`
    [person=3, number=sg] -> ``
  }
}
```

Here `harmony` is a `phonrule` that applies vowel harmony across the concatenated result. The phonrule is applied after all slots are concatenated, with morpheme boundaries (`+`) marking where sound changes can propagate. See [Phonological Rules](08-phonrules.md) for details.

### Example: Turkish-style Agglutination

```
inflection turkish_verb for {tense, person, number} {
  requires stems: root

  compose root + tense_sfx + pn_sfx

  slot tense_sfx {
    [tense=present]  -> `iyor`
    [tense=past]     -> `di`
    [tense=aorist]   -> `r`
  }

  slot pn_sfx {
    [person=1, number=sg] -> `um`
    [person=2, number=sg] -> `sun`
    [person=3, number=sg] -> ``
    [person=1, number=pl] -> `uz`
    [person=2, number=pl] -> `sunuz`
    [person=3, number=pl] -> `lar`
  }
}
```

For the entry `gel` ("to come"):
- present 1sg: `gel` + `iyor` + `um` = `geliyorum`
- past 3sg: `gel` + `di` + `` = `geldi`

### Overrides

`override` rules take priority over compose for specific cells:

```
override [tense=past, person=3, number=sg] -> `{root}tta`
```

This is useful for irregular forms within an otherwise regular agglutinative pattern.

## Delegation

A rule can **delegate** to another inflection class, passing through or fixing tag values and mapping stems:

```
[<conditions>] -> <target>[<tag_args>] with stems { <mapping> }
```

### Tag Arguments

Each tag argument is either:
- **Fixed**: `case=nominative` — always use this value for the delegate
- **Pass-through**: `case` — forward the caller's value for this axis

### Stem Mapping

`with stems { target_stem: source_stem }` maps the delegate's expected stem names to the caller's stems. The source can also be a literal string: `with stems { root: "fixed_value" }`.

### Example: Adjective Declension by Gender

An adjective declines differently by gender. Instead of writing all gender x case x number combinations in one paradigm, delegate to gender-specific sub-paradigms:

```
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

Here `adj_strong` has a paradigm space of case x number x gender (4 x 2 x 3 = 24 cells). Each cell delegates to the appropriate gender-specific paradigm, passing through `case` and `number` and mapping the `root` stem.

### Delegation with Fixed Values

You can also fix specific tag values in the delegation:

```
inflection mediopassive for {tense, voice, person, number} {
  requires stems: root

  [voice=active, _]  -> active_verb[tense, person, number] with stems { root: root }
  [voice=middle, _]  -> middle_verb[tense, person, number] with stems { root: root }
  [voice=passive, _] -> passive_verb[tense, person, number] with stems { root: root }
}
```

### Compiler Validations for Delegation

The compiler checks:
- The delegate target inflection exists
- The `delegate_tag_list` covers all axes in the target's `for { }`
- `with stems` satisfies the target's `requires stems`
- Pass-through axes exist in the caller's `for { }`

## Inline Inflection

Entries can define inflection rules inline with `inflect for { }` instead of referencing a named class. This is useful for one-off irregular words:

```
entry wesen {
  headword: "wesen"
  tags: [parts_of_speech=verb]
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

Inline inflection follows all the same rules as named inflection classes. Note that `stems {}` can be empty when forms are specified as literal templates.

## Forms Override

Entries using `inflection_class` can override specific cells without rewriting the entire paradigm:

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

Override rules are appended to the class's rules and can match with higher specificity to replace individual cells.

`forms_override` is only valid with `inflection_class`, not with inline `inflect`.
