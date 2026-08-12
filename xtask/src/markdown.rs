//! Just enough Markdown to read the tables a registry page is written as.
//!
//! The registries live as pipe tables in prose pages, so syncing them means reading those tables.
//! A general Markdown parser would be a large dependency for one shape of one construct, and the
//! pages are written to a style guide that keeps that shape predictable.

/// One pipe table, and the heading it sits under.
pub struct Table {
    /// The nearest heading above the table, which is how the roles page separates its sections.
    pub heading: String,
    /// The header row's cells.
    pub header: Vec<String>,
    /// Every body row's cells.
    pub rows: Vec<Vec<String>>,
}

impl Table {
    /// Whether the first header cell is `name`, which is how each extractor picks its table.
    pub fn headed_by(&self, name: &str) -> bool {
        self.header.first().is_some_and(|first| first == name)
    }
}

/// Splits a table row into trimmed cells.
fn cells(line: &str) -> Vec<String> {
    line.trim().trim_matches('|').split('|').map(|cell| cell.trim().to_owned()).collect()
}

/// Whether a line is the `| --- | --- |` rule under a header row.
fn is_rule(line: &str) -> bool {
    let line = line.trim();
    !line.is_empty() && line.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

/// Reads every pipe table in a page.
pub fn tables(text: &str) -> Vec<Table> {
    let lines: Vec<&str> = text.lines().collect();
    let mut tables = Vec::new();
    let mut heading = String::new();
    let mut index = 0;

    while index < lines.len() {
        let line = lines[index];
        if line.starts_with('#') {
            line.trim_start_matches('#').trim().clone_into(&mut heading);
        }
        // A table is a header row followed by a rule; anything else starting with `|` is prose.
        let is_table = line.starts_with('|')
            && lines.get(index + 1).is_some_and(|next| next.starts_with('|') && is_rule(next));
        if !is_table {
            index += 1;
            continue;
        }

        let header = cells(line);
        let mut rows = Vec::new();
        index += 2;
        while index < lines.len() && lines[index].starts_with('|') {
            rows.push(cells(lines[index]));
            index += 1;
        }
        tables.push(Table { heading: heading.clone(), header, rows });
    }
    tables
}

/// Every `` `code span` `` in a cell, in order.
///
/// The registries put the value in code spans and the explanation around them, so this is how a
/// cell's data is told from its prose.
pub fn codes(cell: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = cell;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else { break };
        found.push(after[..close].to_owned());
        rest = &after[close + 1..];
    }
    found
}

/// The single code span a cell holds, or an error naming the cell that broke the expectation.
pub fn one_code(cell: &str) -> Result<String, String> {
    match codes(cell).as_slice() {
        [only] => Ok(only.clone()),
        other => Err(format!("expected exactly one code span in {cell:?}, found {}", other.len())),
    }
}

/// A cell as plain text: Markdown links flattened to their label, code spans unquoted.
///
/// What ends up here becomes a doc comment, where a half-rendered link helps nobody.
pub fn prose(cell: &str) -> String {
    // Two passes rather than one, because a link label can itself contain a code span — which is
    // how these pages write a cross-reference to a field, as [`source.fingerprint`](...). Dropping
    // backticks while walking the label would have to know it was inside one; doing it afterwards
    // does not.
    let mut flattened = String::with_capacity(cell.len());
    let mut chars = cell.chars().peekable();

    while let Some(c) = chars.next() {
        if c != '[' {
            flattened.push(c);
            continue;
        }
        // `[label](target)` keeps the label and drops the target.
        for inner in chars.by_ref() {
            if inner == ']' {
                break;
            }
            flattened.push(inner);
        }
        if chars.peek() == Some(&'(') {
            for inner in chars.by_ref() {
                if inner == ')' {
                    break;
                }
            }
        }
    }
    flattened.replace('`', "").trim().to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_a_table_and_the_heading_above_it() {
        let page = "## Common\n\n| Role | Socket |\n| ---- | ------ |\n| `primary` | The only one. |\n";
        let tables = tables(page);
        assert_eq!(tables.len(), 1);
        assert_eq!(tables[0].heading, "Common");
        assert!(tables[0].headed_by("Role"));
        assert_eq!(tables[0].rows, vec![vec!["`primary`".to_owned(), "The only one.".to_owned()]]);
    }

    #[test]
    fn a_pipe_without_a_rule_under_it_is_not_a_table() {
        assert!(tables("| this is prose that happens to start with a pipe\n").is_empty());
    }

    #[test]
    fn code_spans_are_the_data_and_the_rest_is_prose() {
        assert_eq!(codes("`gb`, also `gameboy`"), vec!["gb", "gameboy"]);
        assert_eq!(one_code("`ps1-mc`").unwrap(), "ps1-mc");
        assert!(one_code("`a` and `b`").is_err());
    }

    #[test]
    fn prose_flattens_links_and_unquotes_code() {
        assert_eq!(prose("The `x` tree."), "The x tree.");
        assert_eq!(prose("see [the roles page](/registries/roles/)"), "see the roles page");
    }

    #[test]
    fn a_code_span_inside_a_link_label_loses_its_backticks_too() {
        // How these pages write a cross-reference to a field, and the case a single pass gets
        // wrong: the backticks are consumed while walking the label, so they survive.
        assert_eq!(
            prose("identified by [`source.fingerprint`](#source-map)."),
            "identified by source.fingerprint."
        );
    }
}
