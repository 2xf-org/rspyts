//! Host-neutral parsing of documentation attached to Rust declarations.
//!
//! Rustdoc is the contract's source documentation, but its Markdown section
//! names are not the public conventions of either generated language. This
//! module separates semantic sections before the Python and TypeScript
//! renderers lower them into Google docstrings and TSDoc, respectively.

use std::collections::BTreeMap;

use rspyts::ir::{DefinitionId, ParamDef, TypeRef};

/// Return-value form used when documenting a generated callable.
#[derive(Clone, Copy)]
pub(crate) enum CallableReturn<'a> {
    /// Do not emit return documentation, as for `__init__` and `close`.
    Omitted,
    /// Render an ordinary host contract type.
    Contract(&'a TypeRef),
    /// Render a stateful resource without treating it as a model definition.
    Resource(&'a str),
}

/// Host-neutral inputs required to document one generated callable.
pub(crate) struct CallableDocumentation<'a> {
    /// Authored Rustdoc, when present.
    pub(crate) docs: Option<&'a str>,
    /// Summary used when the Rust declaration has no documentation.
    pub(crate) fallback_summary: String,
    /// Public parameters in declaration order.
    pub(crate) params: &'a [ParamDef],
    /// Public successful return value.
    pub(crate) returns: CallableReturn<'a>,
    /// Typed error exposed by this callable, when any.
    pub(crate) error: Option<&'a DefinitionId>,
}

/// Documentation sections understood by the host-language renderers.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Documentation {
    /// The opening summary paragraph.
    pub(crate) summary: String,
    /// Additional prose and explicit notes.
    pub(crate) notes: String,
    /// Parameter descriptions keyed by their Rust names.
    pub(crate) parameters: BTreeMap<String, String>,
    /// Description of a successful return value.
    pub(crate) returns: String,
    /// Conditions described by Rustdoc's errors section.
    pub(crate) errors: String,
    /// Example prose or source supplied by the author.
    pub(crate) examples: String,
}

impl Documentation {
    /// Parse normalized Rustdoc text into host-neutral semantic sections.
    #[must_use]
    pub(crate) fn parse(source: Option<&str>) -> Self {
        let Some(source) = source.filter(|source| !source.trim().is_empty()) else {
            return Self::default();
        };

        let mut description = Vec::new();
        let mut notes = Vec::new();
        let mut arguments = Vec::new();
        let mut returns = Vec::new();
        let mut errors = Vec::new();
        let mut examples = Vec::new();
        let mut section = Section::Description;

        for line in source.lines() {
            if let Some((next, unknown_heading)) = section_heading(line) {
                section = next;
                if let Some(heading) = unknown_heading {
                    notes.push(format!("{heading}:"));
                }
                continue;
            }
            match section {
                Section::Description => description.push(line.to_owned()),
                Section::Notes => notes.push(line.to_owned()),
                Section::Arguments => arguments.push(line.to_owned()),
                Section::Returns => returns.push(line.to_owned()),
                Section::Errors => errors.push(line.to_owned()),
                Section::Examples => examples.push(line.to_owned()),
            }
        }

        let (summary, trailing_description) = split_summary(&description);
        append_block(&mut notes, &trailing_description);
        let (parameters, unmatched_arguments) = parse_arguments(&arguments);
        append_block(&mut notes, &unmatched_arguments);

        Self {
            summary: normalize_block(&summary),
            notes: normalize_block(&notes),
            parameters,
            returns: normalize_block(&returns),
            errors: normalize_block(&errors),
            examples: normalize_block(&examples),
        }
    }
}

#[derive(Clone, Copy)]
enum Section {
    Description,
    Notes,
    Arguments,
    Returns,
    Errors,
    Examples,
}

/// Recognize conventional Rustdoc headings and Google-style section labels.
fn section_heading(line: &str) -> Option<(Section, Option<String>)> {
    let trimmed = line.trim();
    let (heading, markdown) = if trimmed.starts_with('#') {
        (trimmed.trim_start_matches('#').trim(), true)
    } else if let Some(heading) = trimmed.strip_suffix(':') {
        (heading.trim(), false)
    } else {
        return None;
    };
    let normalized = heading.to_ascii_lowercase();
    let section = match normalized.as_str() {
        "arguments" | "args" | "parameters" | "params" => Section::Arguments,
        "returns" | "return" | "return value" => Section::Returns,
        "errors" | "raises" | "throws" => Section::Errors,
        "notes" | "note" | "remarks" => Section::Notes,
        "examples" | "example" => Section::Examples,
        _ if markdown => return Some((Section::Notes, Some(heading.to_owned()))),
        _ => return None,
    };
    Some((section, None))
}

/// Split the first prose paragraph from subsequent explanatory paragraphs.
fn split_summary(lines: &[String]) -> (Vec<String>, Vec<String>) {
    let lines = trim_blank_lines(lines);
    let boundary = lines
        .iter()
        .position(|line| line.trim().is_empty())
        .unwrap_or(lines.len());
    let summary = lines[..boundary].to_vec();
    let trailing = lines.get(boundary + 1..).unwrap_or_default().to_vec();
    (summary, trailing)
}

/// Parse common Rustdoc parameter-list spellings without discarding prose.
fn parse_arguments(lines: &[String]) -> (BTreeMap<String, String>, Vec<String>) {
    let mut parameters = BTreeMap::<String, String>::new();
    let mut unmatched = Vec::new();
    let mut current: Option<String> = None;

    for line in trim_blank_lines(lines) {
        if let Some((name, description)) = parameter_entry(&line) {
            parameters
                .entry(name.clone())
                .and_modify(|existing| append_sentence(existing, &description))
                .or_insert(description);
            current = Some(name);
        } else if line.trim().is_empty() {
            current = None;
        } else if let Some(name) = &current {
            append_sentence(
                parameters
                    .get_mut(name)
                    .expect("the current parameter was inserted"),
                line.trim(),
            );
        } else {
            unmatched.push(line);
        }
    }
    (parameters, unmatched)
}

/// Parse one bullet such as ``- `name` - description`` or `name: description`.
fn parameter_entry(line: &str) -> Option<(String, String)> {
    let mut value = line.trim();
    let mut bullet = false;
    if let Some(rest) = value
        .strip_prefix("- ")
        .or_else(|| value.strip_prefix("* "))
    {
        value = rest.trim();
        bullet = true;
    }

    let (name, rest) = if let Some(value) = value.strip_prefix('`') {
        let end = value.find('`')?;
        (&value[..end], &value[end + 1..])
    } else if let Some(end) = value.find(':') {
        (&value[..end], &value[end..])
    } else if bullet {
        let end = value.find([' ', '-'])?;
        (&value[..end], &value[end..])
    } else {
        return None;
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || character == '_')
    {
        return None;
    }
    let description = rest
        .trim_start()
        .strip_prefix([':', '-'])
        .unwrap_or(rest.trim_start())
        .trim()
        .to_owned();
    Some((name.to_owned(), description))
}

/// Remove Rustdoc's link brackets while retaining readable inline-code text.
#[must_use]
pub(crate) fn remove_rustdoc_link_brackets(value: &str) -> String {
    value.replace("[`", "`").replace("`]", "`")
}

/// Make the conventional opening of a Rustdoc errors section read naturally
/// as a host-language exception description.
#[must_use]
pub(crate) fn contextualize_error_description(value: &str) -> String {
    value.strip_prefix("Returns ").map_or_else(
        || value.to_owned(),
        |conditions| format!("The Rust implementation returns {conditions}"),
    )
}

fn normalize_block(lines: &[String]) -> String {
    trim_blank_lines(lines)
        .iter()
        .map(|line| line.trim_end())
        .collect::<Vec<_>>()
        .join("\n")
}

fn trim_blank_lines(lines: &[String]) -> Vec<String> {
    let first = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .unwrap_or(lines.len());
    let last = lines
        .iter()
        .rposition(|line| !line.trim().is_empty())
        .map_or(first, |index| index + 1);
    lines[first..last].to_vec()
}

fn append_block(target: &mut Vec<String>, block: &[String]) {
    let block = trim_blank_lines(block);
    if block.is_empty() {
        return;
    }
    if target.iter().any(|line| !line.trim().is_empty()) {
        target.push(String::new());
    }
    target.extend(block);
}

fn append_sentence(target: &mut String, value: &str) {
    if value.is_empty() {
        return;
    }
    if !target.is_empty() {
        target.push(' ');
    }
    target.push_str(value);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_rustdoc_into_language_neutral_sections() {
        let documentation = Documentation::parse(Some(
            "Perform one operation.\n\nAdditional implementation detail.\n\n# Arguments\n\n- `value` - Value to process.\n  Continued detail.\n\n# Returns\n\nThe processed value.\n\n# Errors\n\nReturns `ExampleError` when validation fails.\n\n# Examples\n\n```text\nexample\n```",
        ));

        assert_eq!(documentation.summary, "Perform one operation.");
        assert_eq!(documentation.notes, "Additional implementation detail.");
        assert_eq!(
            documentation.parameters.get("value").map(String::as_str),
            Some("Value to process. Continued detail.")
        );
        assert_eq!(documentation.returns, "The processed value.");
        assert_eq!(
            documentation.errors,
            "Returns `ExampleError` when validation fails."
        );
        assert_eq!(documentation.examples, "```text\nexample\n```");
    }

    #[test]
    fn removes_only_rustdoc_link_brackets() {
        assert_eq!(
            remove_rustdoc_link_brackets("Call [`decode`] with `bytes`."),
            "Call `decode` with `bytes`."
        );
    }

    #[test]
    fn contextualizes_conventional_rustdoc_error_prose() {
        assert_eq!(
            contextualize_error_description("Returns `ExampleError` when validation fails."),
            "The Rust implementation returns `ExampleError` when validation fails."
        );
    }
}
