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
inflection_decl = "inflection" IDENT "for" "{" axis_list "}" "{" inflection_body "}" ;

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
         | delegation ;

delegation = IDENT "[" delegate_tag_list "]" ("with" "stems" "{" stem_mapping_list "}")? ;

delegate_tag_list  = (delegate_tag ",")* delegate_tag ","? ;
delegate_tag       = IDENT "=" IDENT    (* fixed value *)
                   | IDENT              (* pass-through *) ;

stem_mapping_list  = (stem_mapping ",")* stem_mapping ","? ;
stem_mapping       = IDENT ":" IDENT ;
```

### Compose

```ebnf
compose_body = "compose" slot_ref ("+" slot_ref)* compose_slots override_rules ;

slot_ref = IDENT ;

compose_slots  = ("slot" IDENT "{" rule_list "}")* ;
override_rules = ("override" rule)* ;
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
      | STRING ;
```

### Entry References

```ebnf
entry_ref = qualified_name ("#" IDENT)? ("[" tag_condition_list_plain "]")? ;

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
2. **Inflection validation**: verify `for {}` axes exist, rules only reference declared axes
3. **Entry resolution**: look up inflection classes, build stem maps, expand paradigms, collect links
4. **DAG check**: verify `derived_from` links form an acyclic graph

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
| unknown directive `@<name>` | Only @use, @reference, @extend are valid |
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
    entry_id TEXT NOT NULL UNIQUE,
    headword TEXT NOT NULL,
    meaning TEXT NOT NULL
);
```

### entry_tags

Tag assignments for entries.

```sql
CREATE TABLE entry_tags (
    entry_id TEXT NOT NULL,
    axis TEXT NOT NULL,
    value TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
);
```

### entry_meanings

Polysemous meaning entries (entries with `meanings { }` blocks).

```sql
CREATE TABLE entry_meanings (
    entry_id TEXT NOT NULL,
    meaning_id TEXT NOT NULL,
    meaning_text TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
);
```

### headword_scripts

Multi-script headwords.

```sql
CREATE TABLE headword_scripts (
    entry_id TEXT NOT NULL,
    script_name TEXT NOT NULL,
    script_value TEXT NOT NULL,
    FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
);
```

### forms

Inflected forms — the reverse-lookup table.

```sql
CREATE TABLE forms (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    form_str TEXT NOT NULL,
    entry_id TEXT NOT NULL,
    tags TEXT NOT NULL,         -- comma-separated "axis=value" pairs
    part TEXT,                  -- NULL for continuous forms
    FOREIGN KEY (entry_id) REFERENCES entries(entry_id)
);
```

The `tags` column stores the tag conditions that identify this form (e.g., `tense=present,person=1,number=sg`). The `part` column is `NULL` for normal (continuous) forms; for discontinuous forms (e.g., separable verbs), it identifies which part of the word this row represents.

### links

Inter-entry relationship graph.

```sql
CREATE TABLE links (
    src_entry_id TEXT NOT NULL,
    dst_entry_id TEXT NOT NULL,
    link_type TEXT NOT NULL,
    FOREIGN KEY (src_entry_id) REFERENCES entries(entry_id)
);
```

Link types: `derived_from`, `cognate`, `example`.

### tagaxis_meta

Display metadata for axis values.

```sql
CREATE TABLE tagaxis_meta (
    axis_name TEXT NOT NULL,
    value_name TEXT NOT NULL,
    display_lang TEXT NOT NULL,
    display_text TEXT NOT NULL
);
```

### Indexes

```sql
CREATE INDEX idx_forms_entry ON forms(entry_id);
CREATE INDEX idx_forms_form ON forms(form_str);
CREATE INDEX idx_links_src ON links(src_entry_id);
CREATE INDEX idx_links_dst ON links(dst_entry_id);
CREATE INDEX idx_entry_tags ON entry_tags(entry_id);
CREATE INDEX idx_entry_tags_axis ON entry_tags(axis, value);
```

### Full-Text Search

```sql
CREATE VIRTUAL TABLE entries_fts USING fts5(
    entry_id, headword, meaning,
    content='entries', content_rowid='id'
);
```

### Example Queries

```sql
-- Find an entry by headword
SELECT * FROM entries WHERE headword = 'faren';

-- Full-text search
SELECT * FROM entries_fts WHERE entries_fts MATCH 'travel';

-- Reverse lookup: find which entry produces a given form
SELECT e.headword, f.tags
FROM forms f
JOIN entries e ON f.entry_id = e.entry_id
WHERE f.form_str = 'fore';

-- Get all forms for an entry
SELECT form_str, tags FROM forms WHERE entry_id = 'faren';

-- Get all verbs
SELECT e.* FROM entries e
JOIN entry_tags t ON e.entry_id = t.entry_id
WHERE t.axis = 'parts_of_speech' AND t.value = 'verb';

-- Derivation chain
SELECT dst_entry_id FROM links
WHERE src_entry_id = 'faren' AND link_type = 'derived_from';

-- Display names for axis values
SELECT * FROM tagaxis_meta WHERE axis_name = 'tense';
```
