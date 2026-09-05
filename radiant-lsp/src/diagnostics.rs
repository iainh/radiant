use std::collections::{BTreeMap, BTreeSet};

use radiant_compiler::{Analysis, Span};
use tower_lsp::lsp_types::{Diagnostic, DiagnosticSeverity, NumberOrString, Url};

use crate::{
    DocumentSnapshot, DocumentStore,
    semantic::{SemanticIndex, TemplateReference},
    workspace::WorkspaceIndex,
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
            let semantic = SemanticIndex::new(&analysis.template.nodes, &analysis.template.source);
            let targets = semantic
                .template_references()
                .iter()
                .filter_map(|reference| match reference {
                    TemplateReference::Include { target, .. }
                        if !target.is_empty() && analyses.contains_key(target) =>
                    {
                        Some(target.clone())
                    }
                    _ => None,
                })
                .collect();
            (id.as_str(), targets)
        })
        .collect::<BTreeMap<_, Vec<_>>>();
    let mut result = Vec::new();
    let semantic = SemanticIndex::new(&analysis.template.nodes, source);
    for reference in semantic.template_references() {
        match reference {
            TemplateReference::Include {
                target: referenced,
                target_span,
                fragment,
            } => {
                let target_id = if referenced.is_empty() {
                    template_id
                } else {
                    referenced
                };
                let target = if referenced.is_empty() {
                    Some(analysis)
                } else {
                    analyses.get(referenced).copied()
                };
                let Some(target) = target else {
                    result.push(CrossDiagnostic {
                        span: *target_span,
                        code: MISSING_TEMPLATE,
                        message: format!("template `{referenced}` was not found"),
                    });
                    continue;
                };
                if let Some((fragment, fragment_span)) = fragment {
                    let fragments = target
                        .template
                        .fragments()
                        .into_iter()
                        .filter_map(|section| section.arguments.first()?.static_text())
                        .collect::<BTreeSet<_>>();
                    if !fragments.contains(fragment.as_str()) {
                        result.push(CrossDiagnostic {
                            span: *fragment_span,
                            code: MISSING_FRAGMENT,
                            message: format!(
                                "fragment `{fragment}` was not found in template `{}`",
                                target_id
                            ),
                        });
                    }
                }
                if !referenced.is_empty()
                    && let Some(path) = path_to(&graph, referenced, template_id)
                {
                    let mut cycle = vec![template_id.to_owned()];
                    cycle.extend(path);
                    result.push(CrossDiagnostic {
                        span: *target_span,
                        code: INCLUDE_CYCLE,
                        message: format!("static include cycle: {}", cycle.join(" -> ")),
                    });
                }
            }
            TemplateReference::Tag { target, span } if !analyses.contains_key(target) => {
                result.push(CrossDiagnostic {
                    span: *span,
                    code: MISSING_TAG,
                    message: format!(
                        "user tag `{}` was not found",
                        target.strip_prefix("tags/").unwrap_or(target)
                    ),
                });
            }
            TemplateReference::Tag { .. } => {}
        }
    }
    result
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
        let source = "{#fragment local /}{#include $local /}{#include _id=chosen /}{#include card$present /}{#card /}";
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
