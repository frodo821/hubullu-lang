//! Static HTML site generation from `.hut` files.
//!
//! Given a directory of `.hut` files, renders each to an HTML page,
//! generates an `index.html` with navigation, and a `glossary.html`
//! with all referenced dictionary entries and their inflection tables.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::render::{self, AnnotatedPart, EntryAnnotation, ResolveContext};

/// Display name lookup: maps raw axis/value names to human-readable text.
struct DisplayMap {
    axis: HashMap<String, String>,
    value: HashMap<(String, String), String>,
}

impl DisplayMap {
    fn axis_name<'a>(&'a self, raw: &'a str) -> &'a str {
        self.axis.get(raw).map(|s| s.as_str()).unwrap_or(raw)
    }

    fn value_name<'a>(&'a self, axis: &str, raw: &'a str) -> &'a str {
        self.value.get(&(axis.to_string(), raw.to_string()))
            .map(|s| s.as_str())
            .unwrap_or(raw)
    }
}

// ---------------------------------------------------------------------------
// Navigation
// ---------------------------------------------------------------------------

struct NavEntry {
    title: String,
    /// e.g. `"genesis.html"`
    rel_path: String,
}

// ---------------------------------------------------------------------------
// Directory walking
// ---------------------------------------------------------------------------

fn find_hut_files(dir: &Path) -> Result<Vec<PathBuf>, String> {
    let mut files = Vec::new();
    walk_dir(dir, &mut files)?;
    files.sort();
    Ok(files)
}

fn walk_dir(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("cannot read directory '{}': {}", dir.display(), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("directory entry error: {}", e))?;
        let path = entry.path();
        if path.is_dir() {
            walk_dir(&path, out)?;
        } else if path.extension().map(|e| e == "hut").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// HTML helpers
// ---------------------------------------------------------------------------

fn title_from_path(rel: &Path) -> String {
    rel.file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .to_string()
}

fn html_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Shared CSS & page chrome
// ---------------------------------------------------------------------------

const PAGE_CSS: &str = r#"
*, *::before, *::after { box-sizing: border-box; }
body {
  margin: 0;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  color: #1a1a1a;
  background: #fafafa;
  display: flex;
  min-height: 100vh;
}
nav {
  width: 220px;
  flex-shrink: 0;
  background: #fff;
  border-right: 1px solid #e0e0e0;
  padding: 1.5rem 1rem;
  overflow-y: auto;
  position: sticky;
  top: 0;
  height: 100vh;
}
nav > a {
  display: block;
  font-weight: 700;
  font-size: 1.1rem;
  margin-bottom: 1rem;
  color: #333;
  text-decoration: none;
}
nav > a:hover { color: #0060df; }
nav ul {
  list-style: none;
  padding: 0;
  margin: 0;
}
nav li { margin: 0.25rem 0; }
nav li a {
  color: #555;
  text-decoration: none;
  font-size: 0.95rem;
}
nav li a:hover { color: #0060df; }
nav li.current {
  font-weight: 600;
  color: #0060df;
  font-size: 0.95rem;
}
main {
  flex: 1;
  max-width: 52rem;
  padding: 2rem 2.5rem;
}
h1 {
  font-size: 1.6rem;
  margin: 0 0 1.5rem;
  color: #222;
}
.content p {
  line-height: 1.7;
  margin: 0.6rem 0;
}
.content a.word {
  color: #1a1a1a;
  text-decoration: none;
  border-bottom: 1px dotted #999;
  cursor: help;
  transition: border-color 0.15s, color 0.15s;
}
.content a.word:hover {
  color: #0060df;
  border-bottom-color: #0060df;
}
/* glossary page */
.entry-card {
  margin: 1.5rem 0;
  padding: 1rem 1.2rem;
  background: #fff;
  border: 1px solid #e0e0e0;
  border-radius: 6px;
}
.entry-card h2 {
  font-size: 1.15rem;
  margin: 0 0 0.3rem;
  color: #222;
}
.entry-card .meaning {
  color: #555;
  margin: 0 0 0.8rem;
  font-style: italic;
}
.entry-card .meanings {
  margin: 0 0 0.8rem;
  padding: 0;
  list-style: none;
}
.entry-card .meanings li {
  color: #555;
  font-style: italic;
  margin: 0.15rem 0;
}
.entry-card .meanings li .meaning-id {
  font-style: normal;
  font-weight: 600;
  color: #777;
  font-size: 0.85rem;
}
.entry-card .tags {
  display: flex;
  flex-wrap: wrap;
  gap: 0.3rem;
  margin: 0.3rem 0 0.6rem;
}
.entry-card .tag {
  display: inline-block;
  background: #eef1f5;
  color: #556;
  font-size: 0.78rem;
  padding: 0.15rem 0.45rem;
  border-radius: 3px;
}
.entry-card .etymology {
  color: #666;
  font-size: 0.9rem;
  margin: 0.4rem 0 0.8rem;
}
.entry-card .etymology .proto {
  font-family: "Georgia", serif;
  font-style: italic;
}
.entry-card .etymology .etym-note {
  margin-left: 0.3rem;
}
.entry-card .etymology .etym-links {
  margin-top: 0.2rem;
}
.entry-card table {
  border-collapse: collapse;
  font-size: 0.9rem;
  width: 100%;
  max-width: 100%;
}
.entry-card th, .entry-card td {
  border: 1px solid #ddd;
  padding: 0.3rem 0.6rem;
  text-align: left;
}
.entry-card th {
  background: #f5f5f5;
  font-weight: 600;
  font-size: 0.85rem;
  color: #444;
  vertical-align: middle;
}
.entry-card th[rowspan] {
  border-right: 2px solid #ccc;
}
.entry-card td {
  font-family: "Georgia", serif;
}
.entry-card h3 {
  font-size: 0.95rem;
  margin: 1rem 0 0.3rem;
  color: #555;
  font-weight: 600;
}
@media (max-width: 700px) {
  body { flex-direction: column; }
  nav {
    width: 100%;
    height: auto;
    position: static;
    border-right: none;
    border-bottom: 1px solid #e0e0e0;
  }
  nav ul { display: flex; flex-wrap: wrap; gap: 0.5rem; }
  .entry-card { overflow-x: auto; }
}
"#;

fn nav_html(nav: &[NavEntry], current_rel: &str) -> String {
    let mut html = String::new();
    html.push_str("<nav>\n<a href=\"index.html\">Index</a>\n<ul>\n");
    // glossary link
    if current_rel == "glossary.html" {
        html.push_str("<li class=\"current\">Glossary</li>\n");
    } else {
        html.push_str("<li><a href=\"glossary.html\">Glossary</a></li>\n");
    }
    for entry in nav {
        if entry.rel_path == current_rel {
            html.push_str(&format!(
                "<li class=\"current\">{}</li>\n",
                html_escape(&entry.title)
            ));
        } else {
            html.push_str(&format!(
                "<li><a href=\"{}\">{}</a></li>\n",
                html_escape(&entry.rel_path),
                html_escape(&entry.title)
            ));
        }
    }
    html.push_str("</ul>\n</nav>");
    html
}

fn wrap_page(title: &str, body_html: &str, nav: &[NavEntry], current_rel: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>{title}</title>
<style>{css}</style>
</head>
<body>
{nav}
<main>
<h1>{title}</h1>
<div class="content">
{body}
</div>
</main>
</body>
</html>"#,
        title = html_escape(title),
        css = PAGE_CSS,
        nav = nav_html(nav, current_rel),
        body = body_html,
    )
}

// ---------------------------------------------------------------------------
// Text pages (annotated .hut → HTML)
// ---------------------------------------------------------------------------

fn annotated_to_body_html(
    parts: &[AnnotatedPart],
    separator: &str,
    no_sep_before: &str,
) -> String {
    let mut lines: Vec<Vec<HtmlSegment>> = vec![Vec::new()];
    let mut glue_next = false;

    for part in parts {
        match part {
            AnnotatedPart::Glue => {
                glue_next = true;
            }
            AnnotatedPart::Newline => {
                lines.push(Vec::new());
                glue_next = false;
            }
            AnnotatedPart::Lit(text) => {
                let need_sep = needs_separator(
                    lines.last().map(|l| !l.is_empty()).unwrap_or(false),
                    glue_next, text, separator, no_sep_before,
                );
                let line = lines.last_mut().unwrap();
                if need_sep { line.push(HtmlSegment::Sep); }
                line.push(HtmlSegment::Lit(text.clone()));
                glue_next = false;
            }
            AnnotatedPart::Entry { text, annotation } => {
                let need_sep = needs_separator(
                    lines.last().map(|l| !l.is_empty()).unwrap_or(false),
                    glue_next, text, separator, no_sep_before,
                );
                let line = lines.last_mut().unwrap();
                if need_sep { line.push(HtmlSegment::Sep); }
                line.push(HtmlSegment::Entry {
                    text: text.clone(),
                    annotation: annotation.clone(),
                });
                glue_next = false;
            }
        }
    }

    let mut html = String::new();
    for line in &lines {
        if line.is_empty() { continue; }
        html.push_str("<p>");
        for seg in line {
            match seg {
                HtmlSegment::Sep => html.push_str(&html_escape(separator)),
                HtmlSegment::Lit(t) => html.push_str(&html_escape(t)),
                HtmlSegment::Entry { text, annotation } => {
                    let tip = tooltip_text(annotation);
                    html.push_str(&format!(
                        "<a href=\"glossary.html#entry-{}\" class=\"word\" title=\"{}\">{}</a>",
                        html_escape(&annotation.entry_name),
                        html_escape(&tip),
                        html_escape(text),
                    ));
                }
            }
        }
        html.push_str("</p>\n");
    }
    html
}

enum HtmlSegment {
    Sep,
    Lit(String),
    Entry { text: String, annotation: EntryAnnotation },
}

fn needs_separator(
    has_prior: bool, glue: bool, text: &str, separator: &str, no_sep_before: &str,
) -> bool {
    if !has_prior || separator.is_empty() || glue { return false; }
    !text.starts_with(|c: char| no_sep_before.contains(c))
}

fn tooltip_text(ann: &EntryAnnotation) -> String {
    let mut tip = format!("{} — {}", ann.headword, ann.meaning);
    if let Some(tags) = &ann.form_tags {
        tip.push_str(&format!(" [{}]", tags));
    }
    tip
}

fn collect_glossary(parts: &[AnnotatedPart]) -> HashMap<String, EntryAnnotation> {
    let mut seen = HashMap::new();
    for part in parts {
        if let AnnotatedPart::Entry { annotation, .. } = part {
            seen.entry(annotation.entry_name.clone())
                .or_insert_with(|| annotation.clone());
        }
    }
    seen
}

// ---------------------------------------------------------------------------
// Glossary page — inflection / declension tables
// ---------------------------------------------------------------------------

/// Parse a tags string like `"case=nom,number=sg"` into key-value pairs.
fn parse_tags(tags: &str) -> Vec<(String, String)> {
    tags.split(',')
        .filter(|s| !s.is_empty())
        .filter_map(|pair| {
            let mut parts = pair.splitn(2, '=');
            Some((parts.next()?.to_string(), parts.next()?.to_string()))
        })
        .collect()
}

/// A parsed form: the surface string and its tag key-value pairs.
type ParsedForm = (String, Vec<(String, String)>);

/// Maximum values per axis shown in a single table section.
const MAX_ROW_GROUP: usize = 6;
const MAX_COL_GROUP: usize = 6;

/// Build inflection table(s) for a set of forms.
///
/// Strategy for N-dimensional paradigms:
///   1. Collect all axes and their distinct values.
///   2. Pick one axis for **columns** (fewest values, preferring "number"/"gender").
///   3. Remaining axes become **rows** (most-valued outermost, with `rowspan`
///      merging), or **split axes** (if more than 2 row axes).
///   4. Primary row values are chunked at [`MAX_ROW_GROUP`] (6), column
///      values at [`MAX_COL_GROUP`] (6); excess produces sub-tables.
fn build_inflection_table(
    forms: &[(String, String)],
    dm: &DisplayMap,
    def_order: &HashMap<String, Vec<String>>,
) -> Option<String> {
    if forms.is_empty() { return None; }

    let parsed: Vec<ParsedForm> = forms
        .iter()
        .map(|(form, tags)| (form.clone(), parse_tags(tags)))
        .collect();

    // Collect axes → ordered distinct values.
    // Use definition order from tagaxis_meta when available, then append
    // any values not covered by the definition (fallback to data order).
    let mut axis_values: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (_, tags) in &parsed {
        for (k, v) in tags {
            let vals = axis_values.entry(k.clone()).or_default();
            if !vals.contains(v) {
                vals.push(v.clone());
            }
        }
    }
    // Re-sort values according to definition order.
    for (axis, vals) in axis_values.iter_mut() {
        if let Some(order) = def_order.get(axis) {
            vals.sort_by_key(|v| {
                order.iter().position(|o| o == v).unwrap_or(usize::MAX)
            });
        }
    }

    if axis_values.is_empty() { return None; }

    // 1-axis: simple two-column table (chunked at MAX_ROW_GROUP).
    if axis_values.len() == 1 {
        let (axis, vals) = axis_values.iter().next().unwrap();
        let mut html = String::new();
        for chunk in vals.chunks(MAX_ROW_GROUP) {
            html.push_str(&render_single_axis_table(axis, chunk, &parsed, dm));
        }
        return Some(html);
    }

    // Classify axes into column, row, and split roles.
    let (col_axis, row_axes, split_axes) = assign_axis_roles(&axis_values);

    let col_vals = &axis_values[&col_axis];

    // Enumerate distinct split-key combinations.
    let split_keys = enumerate_split_keys(&parsed, &split_axes, &axis_values);

    let mut html = String::new();

    for split_key in &split_keys {
        // Sub-heading for split axes.
        if !split_key.is_empty() {
            let label: String = split_key.iter()
                .map(|(k, v)| format!("{}: {}", dm.axis_name(k), dm.value_name(k, v)))
                .collect::<Vec<_>>()
                .join(" / ");
            html.push_str(&format!("<h3>{}</h3>\n", html_escape(&label)));
        }

        // Filter forms matching this split key.
        let subset: Vec<&ParsedForm> = parsed.iter()
            .filter(|(_, tags)| {
                split_key.iter().all(|(sk, sv)| {
                    tags.iter().any(|(k, v)| k == sk && v == sv)
                })
            })
            .collect();

        // Chunk primary row axis values (≤ MAX_ROW_GROUP per sub-table).
        let primary_row_chunks: Vec<Vec<String>> = if !row_axes.is_empty() {
            axis_values[&row_axes[0]]
                .chunks(MAX_ROW_GROUP)
                .map(|c| c.to_vec())
                .collect()
        } else {
            vec![Vec::new()]
        };

        // Chunk column values (≤ MAX_COL_GROUP per sub-table).
        let col_chunks: Vec<&[String]> = col_vals.chunks(MAX_COL_GROUP).collect();
        let need_chunk_heading = primary_row_chunks.len() > 1 || col_chunks.len() > 1;

        for row_chunk in &primary_row_chunks {
            for col_chunk in &col_chunks {
                // Sub-heading when chunking produces multiple tables.
                if need_chunk_heading {
                    let mut parts = Vec::new();
                    if primary_row_chunks.len() > 1 && !row_chunk.is_empty() {
                        let first = dm.value_name(&row_axes[0], &row_chunk[0]);
                        let last = dm.value_name(&row_axes[0], row_chunk.last().unwrap());
                        if first == last {
                            parts.push(first.to_string());
                        } else {
                            parts.push(format!("{} – {}", first, last));
                        }
                    }
                    if col_chunks.len() > 1 {
                        let first = dm.value_name(&col_axis, &col_chunk[0]);
                        let last = dm.value_name(&col_axis, col_chunk.last().unwrap());
                        if first == last {
                            parts.push(first.to_string());
                        } else {
                            parts.push(format!("{} – {}", first, last));
                        }
                    }
                    if !parts.is_empty() {
                        html.push_str(&format!("<h3>{}</h3>\n", html_escape(&parts.join(" / "))));
                    }
                }

                // Build full cartesian-product row keys, scoped to this chunk.
                let mut chunk_av = axis_values.clone();
                if !row_axes.is_empty() {
                    chunk_av.insert(row_axes[0].clone(), row_chunk.clone());
                }
                let row_keys = enumerate_row_keys_full(&row_axes, &chunk_av);

                let num_row_th = row_axes.len();

                // ---- thead ----
                html.push_str("<table>\n<thead><tr>");
                if num_row_th > 0 {
                    html.push_str(&format!("<th colspan=\"{}\"></th>", num_row_th));
                }
                for cv in *col_chunk {
                    html.push_str(&format!("<th>{}</th>",
                        html_escape(dm.value_name(&col_axis, cv))));
                }
                html.push_str("</tr></thead>\n<tbody>\n");

                // ---- tbody with rowspan ----
                for (i, row_key) in row_keys.iter().enumerate() {
                    html.push_str("<tr>");

                    // Row header cells — merge consecutive identical prefixes.
                    for (axis_idx, (axis, value)) in row_key.iter().enumerate() {
                        let is_first = i == 0
                            || row_keys[i][..=axis_idx] != row_keys[i - 1][..=axis_idx];
                        if is_first {
                            let span = row_keys[i..].iter()
                                .take_while(|rk| rk[..=axis_idx] == row_key[..=axis_idx])
                                .count();
                            if span > 1 {
                                html.push_str(&format!(
                                    "<th rowspan=\"{}\">{}</th>",
                                    span,
                                    html_escape(dm.value_name(axis, value)),
                                ));
                            } else {
                                html.push_str(&format!(
                                    "<th>{}</th>",
                                    html_escape(dm.value_name(axis, value)),
                                ));
                            }
                        }
                    }

                    // Data cells.
                    for cv in *col_chunk {
                        let mut lookup: Vec<(&str, &str)> = row_key.iter()
                            .map(|(k, v)| (k.as_str(), v.as_str()))
                            .collect();
                        lookup.extend(split_key.iter().map(|(k, v)| (k.as_str(), v.as_str())));
                        lookup.push((col_axis.as_str(), cv.as_str()));
                        let form = find_form(&subset, &lookup);
                        html.push_str(&format!("<td>{}</td>", html_escape(&form)));
                    }
                    html.push_str("</tr>\n");
                }
                html.push_str("</tbody></table>\n");
            }
        }
    }

    Some(html)
}

/// Render a simple 1-axis table (two columns: axis-value and form).
fn render_single_axis_table(axis: &str, vals: &[String], parsed: &[ParsedForm], dm: &DisplayMap) -> String {
    let mut html = String::from("<table>\n<thead><tr>");
    html.push_str(&format!("<th>{}</th><th>Form</th>", html_escape(dm.axis_name(axis))));
    html.push_str("</tr></thead>\n<tbody>\n");
    for v in vals {
        let form = find_form_all(parsed, &[(axis, v.as_str())]);
        html.push_str(&format!(
            "<tr><th>{}</th><td>{}</td></tr>\n",
            html_escape(dm.value_name(axis, v)), html_escape(&form),
        ));
    }
    html.push_str("</tbody></table>\n");
    html
}

/// Decide which axes are columns, rows, and split axes.
///
/// - **Column axis** (1): fewest distinct values, preferring "number"/"gender".
/// - **Row axes** (up to 2): most-valued outermost, with `rowspan` merging.
///   Prefers "case"/"tense"/"person".
/// - **Split axes** (rest): each combination gets its own sub-table.
fn assign_axis_roles(
    axis_values: &BTreeMap<String, Vec<String>>,
) -> (String, Vec<String>, Vec<String>) {
    // Axes that prefer to be columns.
    const COL_PREFER: &[&str] = &["number", "gender"];
    // Axes that prefer to be row (inner, not split).
    const ROW_PREFER: &[&str] = &["case", "tense", "person"];

    // Sort axes by number of distinct values (ascending).
    let mut sorted: Vec<(String, usize)> = axis_values.iter()
        .map(|(k, v)| (k.clone(), v.len()))
        .collect();
    sorted.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));

    // Pick column axis: prefer COL_PREFER, then fewest values.
    let col_axis = COL_PREFER.iter()
        .find(|&&p| axis_values.contains_key(p))
        .map(|&s| s.to_string())
        .unwrap_or_else(|| sorted[0].0.clone());

    let remaining: Vec<(String, usize)> = sorted.into_iter()
        .filter(|(k, _)| *k != col_axis)
        .collect();

    if remaining.len() <= 2 {
        let mut row_axes: Vec<String> = remaining.into_iter().map(|(k, _)| k).collect();
        // Most values first → outermost row axis.
        row_axes.sort_by_key(|k| std::cmp::Reverse(axis_values[k].len()));
        return (col_axis, row_axes, Vec::new());
    }

    // More than 2 remaining axes — need to split.
    // Pick up to 2 row axes (prefer ROW_PREFER, then most values).
    let mut row_axes = Vec::new();
    let mut split_axes = Vec::new();

    let mut unassigned: Vec<(String, usize)> = remaining;
    for &pref in ROW_PREFER {
        if row_axes.len() >= 2 { break; }
        if let Some(idx) = unassigned.iter().position(|(k, _)| k == pref) {
            row_axes.push(unassigned.remove(idx).0);
        }
    }
    while row_axes.len() < 2 && !unassigned.is_empty() {
        // Pick the axis with the most values from the remaining.
        let idx = unassigned.iter().enumerate()
            .max_by_key(|(_, (_, n))| *n)
            .map(|(i, _)| i)
            .unwrap();
        row_axes.push(unassigned.remove(idx).0);
    }
    for (k, _) in unassigned {
        split_axes.push(k);
    }
    // Most values first → outermost row axis.
    row_axes.sort_by_key(|k| std::cmp::Reverse(axis_values[k].len()));

    (col_axis, row_axes, split_axes)
}

/// Enumerate distinct combinations of split-axis values, preserving
/// the order they first appear in the data.
fn enumerate_split_keys(
    parsed: &[ParsedForm],
    split_axes: &[String],
    axis_values: &BTreeMap<String, Vec<String>>,
) -> Vec<Vec<(String, String)>> {
    if split_axes.is_empty() {
        return vec![Vec::new()];
    }
    // Cartesian product of split axis values, in their natural order.
    let mut combos: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for axis in split_axes {
        let vals = &axis_values[axis];
        let mut next = Vec::new();
        for combo in &combos {
            for v in vals {
                let mut c = combo.clone();
                c.push((axis.clone(), v.clone()));
                next.push(c);
            }
        }
        combos = next;
    }
    // Filter to only combos that actually have data.
    combos.retain(|combo| {
        parsed.iter().any(|(_, tags)| {
            combo.iter().all(|(ck, cv)| {
                tags.iter().any(|(k, v)| k == ck && v == cv)
            })
        })
    });
    combos
}

/// Enumerate full cartesian-product row keys from axis values.
///
/// Unlike split keys, row keys are **not** filtered — empty combinations
/// render as "—" cells, keeping the table regular for `rowspan` merging.
fn enumerate_row_keys_full(
    row_axes: &[String],
    axis_values: &BTreeMap<String, Vec<String>>,
) -> Vec<Vec<(String, String)>> {
    if row_axes.is_empty() {
        return vec![Vec::new()];
    }
    let mut combos: Vec<Vec<(String, String)>> = vec![Vec::new()];
    for axis in row_axes {
        let vals = &axis_values[axis];
        let mut next = Vec::new();
        for combo in &combos {
            for v in vals {
                let mut c = combo.clone();
                c.push((axis.clone(), v.clone()));
                next.push(c);
            }
        }
        combos = next;
    }
    combos
}

/// Find the form matching a set of tag conditions (searching all parsed forms).
fn find_form_all(parsed: &[ParsedForm], conditions: &[(&str, &str)]) -> String {
    for (form, tags) in parsed {
        let all_match = conditions.iter().all(|(ck, cv)| {
            tags.iter().any(|(k, v)| k == ck && v == cv)
        });
        if all_match {
            return form.clone();
        }
    }
    String::from("—")
}

/// Find the form matching a set of tag conditions (searching a subset of refs).
fn find_form(subset: &[&ParsedForm], conditions: &[(&str, &str)]) -> String {
    for (form, tags) in subset {
        let all_match = conditions.iter().all(|(ck, cv)| {
            tags.iter().any(|(k, v)| k == ck && v == cv)
        });
        if all_match {
            return form.clone();
        }
    }
    String::from("—")
}

fn render_glossary_page(
    entries: &BTreeMap<String, EntryAnnotation>,
    contexts: &[&ResolveContext],
    nav: &[NavEntry],
) -> String {
    // Build display map from all contexts.
    let mut axis_display = HashMap::new();
    let mut value_display = HashMap::new();
    for ctx in contexts {
        let (ad, vd) = ctx.query_tag_display();
        for (k, v) in ad { axis_display.entry(k).or_insert(v); }
        for (k, v) in vd { value_display.entry(k).or_insert(v); }
    }
    let dm = DisplayMap { axis: axis_display, value: value_display };

    // Build axis value definition order from all contexts.
    let mut axis_value_order: HashMap<String, Vec<String>> = HashMap::new();
    for ctx in contexts {
        for (axis, vals) in ctx.query_axis_value_order() {
            let entry = axis_value_order.entry(axis).or_default();
            for v in vals {
                if !entry.contains(&v) {
                    entry.push(v);
                }
            }
        }
    }

    let mut body = String::new();

    for (_, ann) in entries {
        body.push_str(&format!(
            "<div class=\"entry-card\" id=\"entry-{}\">\n<h2>{}</h2>\n",
            html_escape(&ann.entry_name),
            html_escape(&ann.headword),
        ));

        // Tags
        let mut tags = Vec::new();
        for ctx in contexts {
            tags = ctx.query_entry_tags(&ann.entry_name);
            if !tags.is_empty() { break; }
        }
        if !tags.is_empty() {
            body.push_str("<div class=\"tags\">");
            for (axis, value) in &tags {
                let display_val = dm.value_name(axis, value);
                body.push_str(&format!(
                    "<span class=\"tag\">{}</span>",
                    html_escape(display_val),
                ));
            }
            body.push_str("</div>\n");
        }

        // Meanings
        let mut meanings = Vec::new();
        for ctx in contexts {
            meanings = ctx.query_meanings(&ann.entry_name);
            if !meanings.is_empty() { break; }
        }
        if meanings.is_empty() {
            // Single meaning
            body.push_str(&format!(
                "<p class=\"meaning\">{}</p>\n",
                html_escape(&ann.meaning),
            ));
        } else {
            body.push_str("<ul class=\"meanings\">\n");
            for (mid, mtext) in &meanings {
                body.push_str(&format!(
                    "<li><span class=\"meaning-id\">{}</span> {}</li>\n",
                    html_escape(mid),
                    html_escape(mtext),
                ));
            }
            body.push_str("</ul>\n");
        }

        // Etymology
        let mut etymology = (None, None);
        let mut etym_links = Vec::new();
        for ctx in contexts {
            let ety = ctx.query_etymology(&ann.entry_name);
            if ety.0.is_some() || ety.1.is_some() {
                etymology = ety;
                break;
            }
        }
        for ctx in contexts {
            let links = ctx.query_links(&ann.entry_name);
            let filtered: Vec<_> = links.into_iter()
                .filter(|(_, lt)| lt == "derived_from" || lt == "cognate")
                .collect();
            if !filtered.is_empty() {
                etym_links = filtered;
                break;
            }
        }
        if etymology.0.is_some() || etymology.1.is_some() || !etym_links.is_empty() {
            body.push_str("<div class=\"etymology\">");
            if let Some(proto) = &etymology.0 {
                body.push_str(&format!(
                    "<span class=\"proto\">{}</span>",
                    html_escape(proto),
                ));
            }
            if let Some(note) = &etymology.1 {
                body.push_str(&format!(
                    "<span class=\"etym-note\">{}</span>",
                    html_escape(note),
                ));
            }
            if !etym_links.is_empty() {
                body.push_str("<div class=\"etym-links\">");
                for (dst_name, link_type) in &etym_links {
                    let label = if link_type == "derived_from" { "← " } else { "cf. " };
                    body.push_str(&format!(
                        "{}<a href=\"#entry-{}\">{}</a> ",
                        label,
                        html_escape(dst_name),
                        html_escape(dst_name),
                    ));
                }
                body.push_str("</div>");
            }
            body.push_str("</div>\n");
        }

        // Query forms from any available context.
        let mut forms = Vec::new();
        for ctx in contexts {
            forms = ctx.query_forms(&ann.entry_name);
            if !forms.is_empty() { break; }
        }

        if let Some(table_html) = build_inflection_table(&forms, &dm, &axis_value_order) {
            body.push_str(&table_html);
        }

        body.push_str("</div>\n");
    }

    wrap_page("Glossary", &body, nav, "glossary.html")
}

// ---------------------------------------------------------------------------
// Index page
// ---------------------------------------------------------------------------

fn render_index(nav: &[NavEntry]) -> String {
    let mut list_html = String::new();
    list_html.push_str("<li><a href=\"glossary.html\">Glossary</a></li>\n");
    for entry in nav {
        list_html.push_str(&format!(
            "<li><a href=\"{}\">{}</a></li>\n",
            html_escape(&entry.rel_path),
            html_escape(&entry.title),
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Dictionary</title>
<style>
body {{
  margin: 2rem auto;
  max-width: 40rem;
  font-family: -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, "Helvetica Neue", Arial, sans-serif;
  color: #1a1a1a;
  background: #fafafa;
}}
h1 {{ font-size: 1.6rem; }}
ul {{ list-style: none; padding: 0; }}
li {{ margin: 0.4rem 0; }}
a {{ color: #0060df; text-decoration: none; }}
a:hover {{ text-decoration: underline; }}
</style>
</head>
<body>
<h1>Dictionary</h1>
<ul>
{list_html}
</ul>
</body>
</html>"#,
        list_html = list_html,
    )
}

// ---------------------------------------------------------------------------
// Site orchestration
// ---------------------------------------------------------------------------

/// Render all `.hut` files under `dir` to HTML pages in `outdir`.
pub fn render_site(dir: &Path, outdir: &Path, huc: Option<&Path>) -> Result<(), String> {
    let dir = dir
        .canonicalize()
        .map_err(|e| format!("cannot resolve '{}': {}", dir.display(), e))?;

    let hut_files = find_hut_files(&dir)?;
    if hut_files.is_empty() {
        return Err(format!("no .hut files found under '{}'", dir.display()));
    }

    // Build navigation entries.
    let nav: Vec<NavEntry> = hut_files
        .iter()
        .map(|p| {
            let rel = p.strip_prefix(&dir).unwrap_or(p);
            let html_rel = rel.with_extension("html");
            NavEntry {
                title: title_from_path(rel),
                rel_path: html_rel.to_string_lossy().into_owned(),
            }
        })
        .collect();

    std::fs::create_dir_all(outdir)
        .map_err(|e| format!("cannot create '{}': {}", outdir.display(), e))?;

    // Accumulate glossary entries and resolve contexts across all pages.
    let mut all_entries: BTreeMap<String, EntryAnnotation> = BTreeMap::new();
    let mut all_contexts: Vec<ResolveContext> = Vec::new();

    for (hut_path, nav_entry) in hut_files.iter().zip(nav.iter()) {
        let source = std::fs::read_to_string(hut_path)
            .map_err(|e| format!("cannot read '{}': {}", hut_path.display(), e))?;

        let hut_file = render::parse_hut(&source, &hut_path.to_string_lossy())?;

        let hut_dir = hut_path.parent().unwrap_or(Path::new("."));

        let ctx = if let Some(huc_path) = huc {
            render::ResolveContext::from_huc(&hut_file.references, hut_dir, huc_path)?
        } else {
            render::ResolveContext::from_references(&hut_file.references, hut_dir)?
        };

        let parts = render::resolve_annotated(&hut_file.tokens, &ctx)?;
        let (separator, no_sep_before) = render::read_render_config(&ctx);

        let body_html = annotated_to_body_html(&parts, &separator, &no_sep_before);
        let page_html = wrap_page(&nav_entry.title, &body_html, &nav, &nav_entry.rel_path);

        let out_path = outdir.join(&nav_entry.rel_path);
        if let Some(parent) = out_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("cannot create '{}': {}", parent.display(), e))?;
        }
        std::fs::write(&out_path, page_html)
            .map_err(|e| format!("cannot write '{}': {}", out_path.display(), e))?;

        // Collect glossary entries.
        for (name, ann) in collect_glossary(&parts) {
            all_entries.entry(name).or_insert(ann);
        }
        all_contexts.push(ctx);

        eprintln!("  {}", nav_entry.rel_path);
    }

    // Write glossary page.
    let ctx_refs: Vec<&ResolveContext> = all_contexts.iter().collect();
    let glossary_html = render_glossary_page(&all_entries, &ctx_refs, &nav);
    let glossary_path = outdir.join("glossary.html");
    std::fs::write(&glossary_path, glossary_html)
        .map_err(|e| format!("cannot write '{}': {}", glossary_path.display(), e))?;
    eprintln!("  glossary.html");

    // Write index.
    let index_html = render_index(&nav);
    let index_path = outdir.join("index.html");
    std::fs::write(&index_path, index_html)
        .map_err(|e| format!("cannot write '{}': {}", index_path.display(), e))?;

    eprintln!("Wrote {} pages + glossary to {}", nav.len(), outdir.display());
    Ok(())
}
