use std::collections::{BTreeMap, BTreeSet};

use radiant_compiler::{Analysis, ArgumentValue, BUILT_IN_SECTIONS, Node, Section, Span};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Url};

use crate::{
    DocumentSnapshot, DocumentStore, semantic::opening_name_span, workspace::WorkspaceIndex,
};

const MISSING_TEMPLATE: &str = "E_TEMPLATE_NOT_FOUND";
const MISSING_TAG: &str = "E_TAG_NOT_FOUND";
const MISSING_FRAGMENT: &str = "E_FRAGMENT_NOT_FOUND";
const INCLUDE_CYCLE: &str = "E_INCLUDE_CYCLE";

struct CrossDiagnostic {
    span: Span,
    code: &'static str,
    message: String,
}

struct IncludeReference<'a> {
    target: &'a str,
    fragment: Option<&'a str>,
    target_span: Span,
    fragment_span: Option<Span>,
}

pub(crate) fn diagnostics(
    uri: &Url,
    snapshot: &DocumentSnapshot,
    documents: &DocumentStore,
    workspace: &WorkspaceIndex,
) -> Vec<Diagnostic> {
    let mut result = snapshot
        .analysis
        .diagnostics
        .iter()
        .map(|diagnostic| Diagnostic {
            range: snapshot.line_index.span_to_range(diagnostic.span),
            severity: Some(DiagnosticSeverity::ERROR),
            code: Some(NumberOrString::String(diagnostic.code.into())),
            source: Some("radiant".into()),
            message: diagnostic.message.clone(),
            ..Diagnostic::default()
        })
        .collect::<Vec<_>>();

    let Some(template_id) = workspace.template_id(uri) else {
        return result;
    };
    let mut analyses = workspace
        .analyses(uri)
        .into_iter()
        .map(|(id, analysis)| (id.to_owned(), analysis))
        .collect::<BTreeMap<_, _>>();
    for (open_uri, open) in documents.iter() {
        if workspace.shares_root(uri, open_uri)
            && let Some(id) = workspace.template_id(open_uri)
        {
            analyses.insert(id, &open.analysis);
        }
    }

    result.extend(
        cross_diagnostics(&template_id, &snapshot.analysis, &snapshot.text, &analyses)
            .into_iter()
            .map(|diagnostic| Diagnostic {
                range: snapshot.line_index.span_to_range(diagnostic.span),
                severity: Some(DiagnosticSeverity::ERROR),
                code: Some(NumberOrString::String(diagnostic.code.into())),
                source: Some("radiant".into()),
                message: diagnostic.message,
                ..Diagnostic::default()
            }),
    );
    result
}

fn cross_diagnostics(
    template_id: &str,
    analysis: &Analysis,
    source: &str,
    analyses: &BTreeMap<String, &Analysis>,
) -> Vec<CrossDiagnostic> {
    let graph = analyses
        .iter()
        .map(|(id, analysis)| {
            let targets = include_references(&analysis.template.nodes, &analysis.template.source)
                .into_iter()
                .filter(|reference| analyses.contains_key(reference.target))
                .map(|reference| reference.target.to_owned())
                .collect();
            (id.as_str(), targets)
        })
        .collect::<BTreeMap<_, Vec<_>>>();
    let mut result = Vec::new();
    visit_sections(&analysis.template.nodes, &mut |section| {
        if section.name == "include" {
            let Some(reference) = include_reference(section, source) else {
                return;
            };
            let Some(target) = analyses.get(reference.target) else {
                result.push(CrossDiagnostic {
                    span: reference.target_span,
                    code: MISSING_TEMPLATE,
                    message: format!("template `{}` was not found", reference.target),
                });
                return;
            };
            if let Some(fragment) = reference.fragment {
                let fragments = target
                    .template
                    .fragments()
                    .into_iter()
                    .filter_map(|section| section.arguments.first()?.static_text())
                    .collect::<BTreeSet<_>>();
                if !fragments.contains(fragment) {
                    result.push(CrossDiagnostic {
                        span: reference.fragment_span.unwrap_or(reference.target_span),
                        code: MISSING_FRAGMENT,
                        message: format!(
                            "fragment `{fragment}` was not found in template `{}`",
                            reference.target
                        ),
                    });
                }
            }
            if let Some(path) = path_to(&graph, reference.target, template_id) {
                let mut cycle = vec![template_id.to_owned()];
                cycle.extend(path);
                result.push(CrossDiagnostic {
                    span: reference.target_span,
                    code: INCLUDE_CYCLE,
                    message: format!("static include cycle: {}", cycle.join(" -> ")),
                });
            }
        } else if !BUILT_IN_SECTIONS.contains(&section.name.as_str()) {
            let id = format!("tags/{}", section.name);
            if !analyses.contains_key(&id) {
                result.push(CrossDiagnostic {
                    span: opening_name_span(source, section.span, &section.name),
                    code: MISSING_TAG,
                    message: format!("user tag `{}` was not found", section.name),
                });
            }
        }
    });
    result
}

fn include_references<'a>(nodes: &'a [Node], source: &'a str) -> Vec<IncludeReference<'a>> {
    let mut references = Vec::new();
    visit_sections(nodes, &mut |section| {
        if section.name == "include"
            && let Some(reference) = include_reference(section, source)
        {
            references.push(reference);
        }
    });
    references
}

fn include_reference<'a>(section: &'a Section, source: &'a str) -> Option<IncludeReference<'a>> {
    if section
        .arguments
        .iter()
        .any(|argument| argument.name.as_deref() == Some("_id"))
    {
        return None;
    }
    let argument = section.arguments.first()?;
    if argument.name.is_some() {
        return None;
    }
    let value = match &argument.value {
        ArgumentValue::String(value) | ArgumentValue::Raw(value) => value.as_str(),
        ArgumentValue::Expression(_) => return None,
    };
    if value.starts_with("_id=") {
        return None;
    }
    let raw = source.get(argument.span.start..argument.span.end)?;
    let content_start = usize::from(
        raw.as_bytes()
            .first()
            .is_some_and(|quote| matches!(quote, b'\'' | b'"')),
    );
    let (target, fragment) = value
        .split_once('$')
        .map_or((value, None), |(target, fragment)| (target, Some(fragment)));
    let target_span = Span::new(
        argument.span.start + content_start,
        argument.span.start + content_start + target.len(),
    );
    let fragment_span = fragment.map(|fragment| {
        let start = target_span.end + 1;
        Span::new(start, start + fragment.len())
    });
    Some(IncludeReference {
        target,
        fragment,
        target_span,
        fragment_span,
    })
}

fn visit_sections<'a>(nodes: &'a [Node], visitor: &mut impl FnMut(&'a Section)) {
    for node in nodes {
        if let Node::Section(section) = node {
            visitor(section);
            for block in &section.blocks {
                visit_sections(&block.nodes, visitor);
            }
        }
    }
}

fn path_to(
    graph: &BTreeMap<&str, Vec<String>>,
    start: &str,
    destination: &str,
) -> Option<Vec<String>> {
    fn visit(
        graph: &BTreeMap<&str, Vec<String>>,
        current: &str,
        destination: &str,
        visited: &mut BTreeSet<String>,
    ) -> Option<Vec<String>> {
        if current == destination {
            return Some(vec![current.to_owned()]);
        }
        if !visited.insert(current.to_owned()) {
            return None;
        }
        for next in graph.get(current).into_iter().flatten() {
            if let Some(mut path) = visit(graph, next, destination, visited) {
                path.insert(0, current.to_owned());
                return Some(path);
            }
        }
        None
    }
    visit(graph, start, destination, &mut BTreeSet::new())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use radiant_compiler::{Analysis, analyze};

    use super::cross_diagnostics;

    fn codes(source: &str, others: &[(&str, &str)]) -> Vec<(&'static str, String, (usize, usize))> {
        let current = analyze("page", source);
        let parsed = others
            .iter()
            .map(|(id, source)| ((*id).to_owned(), analyze(*id, *source)))
            .collect::<BTreeMap<String, Analysis>>();
        let mut analyses = parsed
            .iter()
            .map(|(id, analysis)| (id.clone(), analysis))
            .collect::<BTreeMap<_, _>>();
        analyses.insert("page".into(), &current);
        cross_diagnostics("page", &current, source, &analyses)
            .into_iter()
            .map(|diagnostic| {
                (
                    diagnostic.code,
                    diagnostic.message,
                    (diagnostic.span.start, diagnostic.span.end),
                )
            })
            .collect()
    }

    #[test]
    fn reports_missing_static_targets_tags_and_fragments_at_reference_spans() {
        let source = "😀 {#include 'missing' /}{#lost /}{#include card$absent /}";
        assert_eq!(
            codes(source, &[("card", "{#fragment present /}")]),
            [
                (
                    "E_TEMPLATE_NOT_FOUND",
                    "template `missing` was not found".into(),
                    (
                        source.find("missing").unwrap(),
                        source.find("missing").unwrap() + 7
                    )
                ),
                (
                    "E_TAG_NOT_FOUND",
                    "user tag `lost` was not found".into(),
                    (
                        source.find("lost").unwrap(),
                        source.find("lost").unwrap() + 4
                    )
                ),
                (
                    "E_FRAGMENT_NOT_FOUND",
                    "fragment `absent` was not found in template `card`".into(),
                    (
                        source.find("absent").unwrap(),
                        source.find("absent").unwrap() + 6
                    )
                ),
            ]
        );
    }

    #[test]
    fn ignores_dynamic_includes_and_accepts_existing_fragments() {
        let source = "{#include _id=chosen /}{#include card$present /}{#card /}";
        assert!(
            codes(
                source,
                &[("card", "{#capture present /}"), ("tags/card", "")]
            )
            .is_empty()
        );
    }

    #[test]
    fn reports_each_static_include_edge_that_closes_a_workspace_cycle() {
        let source = "{#include middle /}";
        assert_eq!(
            codes(
                source,
                &[("middle", "{#include end /}"), ("end", "{#include page /}")]
            ),
            [(
                "E_INCLUDE_CYCLE",
                "static include cycle: page -> middle -> end -> page".into(),
                (10, 16)
            )]
        );
    }
}
