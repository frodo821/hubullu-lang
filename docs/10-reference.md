# Reference

Complete syntax reference, compilation model, error catalog, and output schema.

## Grammar

### Top-Level

```ebnf
file = item* ;

item = use_decl
     | reference_decl
     | tagaxis_decl
     | extend_decl
     | inflection_decl
     | phonrule_decl
     | render_decl
     | entry_decl ;
```

### Imports

```ebnf
use_decl       = "@use" import_target "from" STRING ;
reference_decl = "@reference" import_target "from" STRING ;

import_target  = "*" ("as" IDENT)?
               | import_list ;

import_list    = "(" import_entries ")"
               | import_entries ;

import_entries = import_entry ("," import_entry)* ","? ;
import_entry   = IDENT ("as" IDENT)? ;
```

### Tag Axis

```ebnf
tagaxis_decl = "tagaxis" IDENT "{" tagaxis_body "}" ;

tagaxis_body = tagaxis_field* ;

tagaxis_field = "role" ":" role_value
              | "display" ":" display_map
              | "index" ":" index_value ;

role_value  = "inflectional" | "classificatory" | "structural" ;
index_value = "exact" | "fulltext" ;
display_map = "{" (IDENT ":" STRING ",")* IDENT ":" STRING ","? "}" ;
```

### @extend

```ebnf
extend_decl = "@extend" IDENT "for" "tagaxis" IDENT "{" extend_value* "}" ;

extend_value = IDENT "{" extend_value_body "}" ;

extend_value_body = "display" ":" display_map
                  | "slots" ":" "[" IDENT ("," IDENT)* ","? "]" ;
```

### Inflection

```ebnf
inflection_decl = "inflection" IDENT display_clause? "for" "{" axis_list "}" "{" inflection_body "}" ;

display_clause = "display" display_map ;

axis_list = IDENT ("," IDENT)* ","? ;

inflection_body = requires_stems? (rule_list | compose_body) ;

requires_stems = "requires" "stems" ":" stem_req ("," stem_req)* ;
stem_req       = IDENT ("[" tag_condition_list_plain "]")? ;

rule_list = rule+ ;
rule      = "[" tag_condition_list "]" "->" rule_rhs ;

tag_condition_list       = (tag_condition ",")* (tag_condition | "_") ","? ;
tag_condition            = IDENT "=" IDENT ;
tag_condition_list_plain = (tag_condition ",")* tag_condition ","? ;

rule_rhs = TEMPLATE
         | "null"
         | delegation
         | phon_apply ;

phon_apply = IDENT "(" rule_rhs ")" ;

delegation = IDENT "[" delegate_tag_list "]" ("with" "stems" "{" stem_mapping_list "}")? ;

delegate_tag_list  = (delegate_tag ",")* delegate_tag ","? ;
delegate_tag       = IDENT "=" IDENT    (* fixed value *)
                   | IDENT              (* pass-through *) ;

stem_mapping_list  = (stem_mapping ",")* stem_mapping ","? ;
stem_mapping       = IDENT ":" IDENT ;
```

### Compose

```ebnf
compose_body = "compose" compose_expr compose_slots override_rules ;

compose_expr = compose_term ("+" compose_term)*
             | IDENT "(" compose_expr ")" ;

compose_term = IDENT ;

compose_slots  = ("slot" IDENT "{" rule_list "}")* ;
override_rules = ("override" rule)* ;
```

### Phonological Rules

```ebnf
phonrule_decl = "phonrule" IDENT "{" phonrule_body "}" ;

phonrule_body = class_def* map_def* rewrite_rule* ;

class_def = "class" IDENT "=" class_body ;
class_body = "[" STRING ("," STRING)* ","? "]"
           | IDENT ("|" IDENT)* ;

map_def = "map" IDENT "=" IDENT "->" "match" "{" map_arm* else_arm? "}" ;
map_arm = STRING "->" (STRING | IDENT) "," ;
else_arm = "else" "->" (STRING | IDENT) ;

rewrite_rule = phon_pattern "->" phon_replacement ("/" phon_context)? ;
phon_pattern = IDENT | STRING ;
phon_replacement = IDENT | STRING | "null" ;
phon_context = context_elem* "_" context_elem* ;
context_elem = IDENT ("*" | "+")?
             | "!" IDENT ("*" | "+")?
             | "+"
             | "^"
             | "$"
             | "(" context_elem ("|" context_elem)* ")"
             | STRING ;
```

### @render

```ebnf
render_decl = "@render" "{" render_field* "}" ;

render_field = "separator" ":" STRING
             | "no_separator_before" ":" STRING ;
```

### Entry

```ebnf
entry_decl = "entry" IDENT "{" entry_body "}" ;

entry_body = entry_field* ;

entry_field = "headword" ":" STRING
            | "headword" "{" headword_scripts "}"
            | "tags" ":" "[" tag_condition_list_plain? "]"
            | "stems" "{" (IDENT ":" STRING)* "}"
            | "inflection_class" ":" IDENT
            | "inflect" "for" "{" axis_list "}" "{" rule_list "}"
            | "meaning" ":" STRING
            | "meanings" "{" meaning_entry+ "}"
            | "forms_override" "{" rule_list "}"
            | "etymology" "{" etymology_body "}"
            | "examples" "{" example+ "}" ;

headword_scripts = (IDENT ":" STRING)+ ;

meaning_entry = IDENT "{" STRING "}" ;
```

### Etymology

```ebnf
etymology_body = etymology_field* ;

etymology_field = "proto" ":" STRING
                | "cognates" "{" cognate_entry+ "}"
                | "derived_from" ":" entry_ref
                | "note" ":" STRING ;

cognate_entry = entry_ref STRING ;
```

### Examples

```ebnf
example = "example" "{" "tokens" ":" token+ "translation" ":" STRING "}" ;

token = entry_ref ("[" tag_condition_list_plain? "]")?
      | entry_ref "[$=" IDENT "]"
      | STRING
      | "~"
      | "//" ;
```

### Entry References

```ebnf
entry_ref = qualified_name ("#" IDENT)? ( "[" tag_condition_list_plain "]" | "[$=" IDENT "]" )? ;

qualified_name = IDENT ("." IDENT)* ;
```

### Lexical Elements

```ebnf
IDENT    = XID_Start XID_Continue*
         | "_" XID_Continue+
         | DIGIT+ ;

STRING   = '"' (escape | [^"\\])* '"' ;
TEMPLATE = '`' (escape | template_interp | [^`\\{}])* '`' ;

template_interp = "{" IDENT ("." IDENT)? "}" ;

escape = "\\" | '\"' | "\n" | "\t"
       | "\{" | "\}" | "\`" ;

COMMENT = "#" [^\n]* ;   (* only when preceded by whitespace *)
```

## Compilation Phases

### Phase 1: Collection

1. Starting from the entry-point file, recursively resolve `@use` directives (DFS with cycle detection)
2. Collect `@reference` paths and load entry files (deduplication; cycles allowed)
3. Lex and parse every loaded file
4. Register all top-level declarations into the symbol table (hoisting)
5. Resolve import bindings: glob imports copy matching symbols; named imports verify existence and kind

### Phase 2: Resolution

1. **@extend resolution**: populate axis values, check for conflicts
2. **Phonrule validation**: verify class and map references in rewrite rules
3. **Inflection validation**: verify `for {}` axes exist, rules only reference declared axes
4. **Entry resolution**: look up inflection classes, build stem maps, expand paradigms, collect links
5. **DAG check**: verify `derived_from` links form an acyclic graph

### Phase 3: Emit

Create SQLite database with tables, indexes, and FTS5 virtual table.

## Error Catalog

### Parse Errors

| Error | Cause |
| ----- | ----- |
| `_` in non-terminal position of tag condition list | Wildcard must be last |
| missing `role` field in tagaxis | Required field |
| unknown role `<name>` | Must be inflectional, classificatory, or structural |
| unknown tagaxis field `<name>` | Unrecognized field |
| missing headword | Entry requires headword |
| missing meaning | Entry requires meaning or meanings |
| unexpected field in entry `<name>` | Unrecognized entry field |
| expected top-level item | Unrecognized syntax at file level |
| unterminated string literal | Missing closing `"` |
| unterminated template literal | Missing closing `` ` `` |
| unknown escape sequence | Unrecognized `\x` |
| missing `}` in template interpolation | `{name` without closing `}` |
| unknown directive `@<name>` | Only @use, @reference, @extend, @render are valid |
| unexpected character `<char>` | Character not part of any token |

### Phase 1 Errors

| Error | Cause |
| ----- | ----- |
| circular @use detected | Mutual @use between files |
| file not found | Import path doesn't resolve |
| cannot import entry via @use | Named @use targeting an entry |
| cannot import declaration via @reference | Named @reference targeting tagaxis/extend/inflection |
| symbol `<name>` not found in `<file>` | Named import references nonexistent symbol |
| duplicate definition of `<name>` | Same name defined twice in one file |

### Phase 2 Errors

| Error | Cause |
| ----- | ----- |
| @extend targets unknown tagaxis `<name>` | Target axis doesn't exist |
| conflicting @extend: value `<val>` already defined | Duplicate value addition |
| axis `<name>` not in for {} declaration | Rule references undeclared axis |
| inflection class `<name>` not found | Entry references nonexistent class |
| no rule matches cell [...] | Paradigm has uncovered cells |
| ambiguous rule match | Two rules match with same specificity |
| undefined stem `<name>` | Template references nonexistent stem |
| undefined structural stem `<name>` | Slot interpolation references nonexistent stem |
| undefined slot `<name>` | Slot interpolation references nonexistent slot |
| cyclic derived_from relationship detected | Derivation links form a cycle |
| axis `<name>` has no defined values | Axis used in for {} but has no @extend values |

## Output Schema (SQLite)

### entries

Main entry table.

```sql
CREATE TABLE entries (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL,
    headword TEXT NOT NULL,
    meaning TEXT NOT NULL,
    inflection_class_id INTEGER
);
```

The `name` column stores the entry's identifier. `inflection_class_id` references `inflection_meta(id)` and is `NULL` for entries without inflection.

### entry_tags

Tag assignments for entries.

```sql
CREATE TABLE entry_tags (
    entry_id INTEGER NOT NULL,
    axis TEXT NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(id)
);
```

### entry_meanings

Polysemous meaning entries (entries with `meanings { }` blocks).

```sql
CREATE TABLE entry_meanings (
    entry_id INTEGER NOT NULL,
    meaning_id TEXT NOT NULL,
    meaning_text TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(id)
);
```

### headword_scripts

Multi-script headwords.

```sql
CREATE TABLE headword_scripts (
    entry_id INTEGER NOT NULL,
    script_name TEXT NOT NULL,
    script_value TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(id)
);
```

### stems

Stem values for entries (used by `[$=name]` references).

```sql
CREATE TABLE stems (
    entry_id INTEGER NOT NULL,
    stem_name TEXT NOT NULL,
    stem_value TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(id)
);
```

### forms

Inflected forms — the reverse-lookup table.

```sql
CREATE TABLE forms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    form_str TEXT NOT NULL,
    entry_id INTEGER NOT NULL,
    tags TEXT NOT NULL,         -- comma-separated "axis=value" pairs
    part TEXT,                  -- NULL for continuous forms
    FOREIGN KEY (entry_id) REFERENCES entries(id)
);
```

The `tags` column stores the tag conditions that identify this form (e.g., `tense=present,person=1,number=sg`). The `part` column is `NULL` for normal (continuous) forms; for discontinuous forms (e.g., separable verbs), it identifies which part of the word this row represents.

### links

Inter-entry relationship graph.

```sql
CREATE TABLE links (
    src_entry_id INTEGER NOT NULL,
    dst_entry_id INTEGER NOT NULL,
    link_type TEXT NOT NULL,
    FOREIGN KEY (src_entry_id) REFERENCES entries(id),
    FOREIGN KEY (dst_entry_id) REFERENCES entries(id)
);
```

Link types: `derived_from`, `cognate`, `example`.

### tagaxis_meta

Display metadata for axis values.

```sql
CREATE TABLE tagaxis_meta (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    axis_name TEXT NOT NULL,
    value_name TEXT NOT NULL,
    display_lang TEXT NOT NULL,
    display_text TEXT NOT NULL
);
```

### inflection_meta

Inflection class metadata.

```sql
CREATE TABLE inflection_meta (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    name TEXT NOT NULL
);
```

### inflection_display

Multilingual display names for inflection classes.

```sql
CREATE TABLE inflection_display (
    inflection_id INTEGER NOT NULL,
    display_lang TEXT NOT NULL,
    display_text TEXT NOT NULL,
    FOREIGN KEY (inflection_id) REFERENCES inflection_meta(id)
);
```

### inflection_axes

Axes declared in each inflection class's `for { }`.

```sql
CREATE TABLE inflection_axes (
    inflection_id INTEGER NOT NULL,
    axis_name TEXT NOT NULL,
    FOREIGN KEY (inflection_id) REFERENCES inflection_meta(id)
);
```

### render_config

Token rendering configuration (from `@render`).

```sql
CREATE TABLE render_config (
    key TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
```

Keys: `separator` (default: `" "`), `no_separator_before` (default: `".,;:!?"`).

### Indexes

```sql
CREATE INDEX idx_entries_name ON entries(name);
CREATE INDEX idx_stems_entry ON stems(entry_id);
CREATE INDEX idx_forms_entry ON forms(entry_id);
CREATE INDEX idx_forms_form ON forms(form_str);
CREATE INDEX idx_links_src ON links(src_entry_id);
CREATE INDEX idx_links_dst ON links(dst_entry_id);
CREATE INDEX idx_entry_tags ON entry_tags(entry_id);
CREATE INDEX idx_entry_tags_axis ON entry_tags(axis, value);
CREATE INDEX idx_inflection_display ON inflection_display(inflection_id);
CREATE INDEX idx_inflection_axes ON inflection_axes(inflection_id);
```

### Full-Text Search

```sql
CREATE VIRTUAL TABLE entries_fts USING fts5(
    name, headword, meaning,
    content='entries', content_rowid='id'
);
```

### Example Queries

```sql
-- Find an entry by name
SELECT * FROM entries WHERE name = 'faren';

-- Full-text search
SELECT * FROM entries_fts WHERE entries_fts MATCH 'travel';

-- Reverse lookup: find which entry produces a given form
SELECT e.headword, f.tags
FROM forms f
JOIN entries e ON f.entry_id = e.id
WHERE f.form_str = 'fore';

-- Get all forms for an entry
SELECT f.form_str, f.tags FROM forms f
JOIN entries e ON f.entry_id = e.id
WHERE e.name = 'faren';

-- Get all verbs
SELECT e.* FROM entries e
JOIN entry_tags t ON e.id = t.entry_id
WHERE t.axis = 'parts_of_speech' AND t.value = 'verb';

-- Derivation chain
SELECT e2.name FROM links l
JOIN entries e1 ON l.src_entry_id = e1.id
JOIN entries e2 ON l.dst_entry_id = e2.id
WHERE e1.name = 'faren' AND l.link_type = 'derived_from';

-- Display names for axis values
SELECT * FROM tagaxis_meta WHERE axis_name = 'tense';

-- Inflection class metadata
SELECT m.name, d.display_lang, d.display_text
FROM inflection_meta m
JOIN inflection_display d ON m.id = d.inflection_id;
```
