use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use tower_lsp::lsp_types::{FileChangeType, FileEvent, Url};

use radiant_compiler::{Analysis, analyze};

use crate::DocumentStore;

#[derive(Debug)]
struct IndexedTemplate {
    path: PathBuf,
    analysis: Option<Analysis>,
}

#[derive(Debug)]
struct TemplateRoot {
    workspace: PathBuf,
    templates: PathBuf,
    files: BTreeMap<String, IndexedTemplate>,
}

impl TemplateRoot {
    fn new(workspace: PathBuf) -> Self {
        let templates = workspace.join("templates");
        let files = discover(&templates);
        Self {
            workspace,
            templates,
            files,
        }
    }

    fn refresh(&mut self) {
        self.files = discover(&self.templates);
    }

    fn update_file(&mut self, path: &Path, change: FileChangeType) {
        if ignored_template_path(&self.templates, path) {
            return;
        }
        let Some(id) = template_id(&self.templates, path) else {
            return;
        };
        if change == FileChangeType::DELETED {
            self.files.remove(&id);
            return;
        }
        let analysis = fs::read_to_string(path)
            .ok()
            .map(|source| analyze(&id, source));
        if path.is_file() {
            self.files.insert(
                id,
                IndexedTemplate {
                    path: path.to_owned(),
                    analysis,
                },
            );
        } else {
            self.files.remove(&id);
        }
    }

    fn requires_refresh(&self, path: &Path, change: FileChangeType) -> bool {
        !ignored_template_path(&self.templates, path)
            && (path.is_dir()
                || (change == FileChangeType::DELETED
                    && self
                        .files
                        .values()
                        .any(|template| template.path != path && template.path.starts_with(path))))
    }
}

/// The templates discovered in each configured workspace folder.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceIndex {
    roots: Vec<TemplateRoot>,
}

pub(crate) struct WorkspaceDocument<'a> {
    pub(crate) id: String,
    pub(crate) uri: Url,
    pub(crate) analysis: &'a Analysis,
}

impl WorkspaceIndex {
    pub(crate) fn set_roots(&mut self, roots: impl IntoIterator<Item = Url>) {
        let mut paths = roots
            .into_iter()
            .filter_map(|uri| uri.to_file_path().ok())
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();
        self.roots = paths.into_iter().map(TemplateRoot::new).collect();
    }

    pub(crate) fn change_roots(
        &mut self,
        added: impl IntoIterator<Item = Url>,
        removed: impl IntoIterator<Item = Url>,
    ) {
        let removed = removed
            .into_iter()
            .filter_map(|uri| uri.to_file_path().ok())
            .collect::<Vec<_>>();
        self.roots.retain(|root| !removed.contains(&root.workspace));

        for workspace in added.into_iter().filter_map(|uri| uri.to_file_path().ok()) {
            if !self.roots.iter().any(|root| root.workspace == workspace) {
                self.roots.push(TemplateRoot::new(workspace));
            }
        }
        self.roots
            .sort_by(|left, right| left.workspace.cmp(&right.workspace));
    }

    pub(crate) fn valid_id(&self, id: &str) -> bool {
        safe_id(id)
    }

    pub(crate) fn documents<'a>(
        &'a self,
        open: &'a DocumentStore,
        anchor: Option<&Url>,
    ) -> Vec<WorkspaceDocument<'a>> {
        let anchor_root = anchor.and_then(|uri| self.root_for_document(uri));
        let mut documents = BTreeMap::new();
        for root in &self.roots {
            if anchor_root.is_some_and(|anchor| anchor.templates != root.templates) {
                continue;
            }
            for (id, template) in &root.files {
                if let Some(analysis) = &template.analysis
                    && let Ok(uri) = Url::from_file_path(&template.path)
                {
                    documents.insert(
                        uri.clone(),
                        WorkspaceDocument {
                            id: id.clone(),
                            uri,
                            analysis,
                        },
                    );
                }
            }
        }
        for (uri, snapshot) in open.iter() {
            let Some(root) = self.root_for_document(uri) else {
                continue;
            };
            if anchor_root.is_some_and(|anchor| anchor.templates != root.templates) {
                continue;
            }
            if let Ok(path) = uri.to_file_path()
                && !ignored_template_path(&root.templates, &path)
                && let Some(id) = template_id(&root.templates, &path)
            {
                documents.insert(
                    uri.clone(),
                    WorkspaceDocument {
                        id,
                        uri: uri.clone(),
                        analysis: &snapshot.analysis,
                    },
                );
            }
        }
        documents.into_values().collect()
    }

    pub(crate) fn update_affected(&mut self, changed: impl IntoIterator<Item = FileEvent>) {
        let changes = changed
            .into_iter()
            .filter_map(|event| event.uri.to_file_path().ok().map(|path| (path, event.typ)))
            .collect::<Vec<_>>();
        for root in &mut self.roots {
            let affected = changes
                .iter()
                .filter(|(path, _)| path.starts_with(&root.templates))
                .collect::<Vec<_>>();
            if affected
                .iter()
                .any(|(path, change)| root.requires_refresh(path, *change))
            {
                root.refresh();
                continue;
            }
            for (path, change) in affected {
                root.update_file(path, *change);
            }
        }
    }

    pub(crate) fn template_id(&self, document: &Url) -> Option<String> {
        let path = document.to_file_path().ok()?;
        let root = self.root_for_document(document)?;
        template_id(&root.templates, &path)
    }

    pub(crate) fn analyses(&self, document: &Url) -> Vec<(&str, &Analysis)> {
        self.root_for_document(document)
            .map(|root| {
                root.files
                    .iter()
                    .filter_map(|(id, template)| {
                        template
                            .analysis
                            .as_ref()
                            .map(|analysis| (id.as_str(), analysis))
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn shares_root(&self, left: &Url, right: &Url) -> bool {
        self.root_for_document(left)
            .zip(self.root_for_document(right))
            .is_some_and(|(left, right)| left.templates == right.templates)
    }

    fn root_for_document(&self, document: &Url) -> Option<&TemplateRoot> {
        let path = document.to_file_path().ok()?;
        self.roots
            .iter()
            .filter(|root| path.starts_with(&root.templates))
            .max_by(|left, right| {
                left.workspace
                    .components()
                    .count()
                    .cmp(&right.workspace.components().count())
                    .then_with(|| right.workspace.cmp(&left.workspace))
            })
    }
}

fn discover(root: &Path) -> BTreeMap<String, IndexedTemplate> {
    fn visit(root: &Path, directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if ignored_template_path(root, &path) {
                continue;
            }
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                visit(root, &path, files);
            } else if file_type.is_file() {
                files.push(path);
            }
        }
    }

    let mut paths = Vec::new();
    visit(root, root, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let id = template_id(root, &path)?;
            let analysis = fs::read_to_string(&path)
                .ok()
                .map(|source| analyze(&id, source));
            Some((id, IndexedTemplate { path, analysis }))
        })
        .collect()
}

fn ignored_template_path(root: &Path, path: &Path) -> bool {
    let Ok(relative) = path.strip_prefix(root) else {
        return true;
    };
    relative.components().any(|component| {
        let Some(name) = component.as_os_str().to_str() else {
            return true;
        };
        name.starts_with('.')
            || name.ends_with('~')
            || (name.starts_with('#') && name.ends_with('#'))
            || [
                ".bak", ".backup", ".orig", ".rej", ".swp", ".swo", ".swx", ".tmp", ".temp",
            ]
            .iter()
            .any(|suffix| name.ends_with(suffix))
    })
}

fn template_id(root: &Path, path: &Path) -> Option<String> {
    path.strip_prefix(root)
        .ok()?
        .with_extension("")
        .components()
        .map(|component| component.as_os_str().to_str())
        .collect::<Option<Vec<_>>>()
        .map(|components| components.join("/"))
}

fn safe_id(id: &str) -> bool {
    !id.is_empty()
        && !id.contains('\\')
        && Path::new(id)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;
    use tower_lsp::lsp_types::{FileChangeType, FileEvent, Url};

    use super::WorkspaceIndex;

    fn file_uri(path: &std::path::Path) -> Url {
        Url::from_file_path(path).unwrap()
    }

    fn event(path: &std::path::Path, typ: FileChangeType) -> FileEvent {
        FileEvent {
            uri: file_uri(path),
            typ,
        }
    }

    #[test]
    fn discovers_normalized_nested_extensionless_ids_and_isolates_roots() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        fs::create_dir_all(first.path().join("templates/nested")).unwrap();
        fs::create_dir_all(first.path().join("templates/tags/admin")).unwrap();
        fs::create_dir_all(second.path().join("templates")).unwrap();
        fs::write(first.path().join("templates/nested/card.html"), "card").unwrap();
        fs::write(first.path().join("templates/tags/admin/badge.txt"), "tag").unwrap();
        fs::write(second.path().join("templates/other.html"), "other").unwrap();

        let mut index = WorkspaceIndex::default();
        index.set_roots([file_uri(second.path()), file_uri(first.path())]);
        let document = file_uri(&first.path().join("templates/page.html"));

        assert_eq!(
            index
                .analyses(&document)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            ["nested/card", "tags/admin/badge"]
        );
        assert!(index.valid_id("nested/card"));
        assert!(!index.valid_id("../outside"));
        assert!(!index.valid_id("/outside"));
    }

    #[test]
    fn refreshes_only_roots_affected_by_watched_create_change_and_delete() {
        let first = tempdir().unwrap();
        let second = tempdir().unwrap();
        fs::create_dir_all(first.path().join("templates")).unwrap();
        fs::create_dir_all(second.path().join("templates")).unwrap();
        let first_document = file_uri(&first.path().join("templates/page.html"));
        let second_document = file_uri(&second.path().join("templates/page.html"));
        let mut index = WorkspaceIndex::default();
        index.set_roots([file_uri(first.path()), file_uri(second.path())]);

        let created = first.path().join("templates/new.html");
        fs::write(&created, "new").unwrap();
        index.update_affected([event(&created, FileChangeType::CREATED)]);
        assert_eq!(index.analyses(&first_document)[0].0, "new");
        assert!(index.analyses(&second_document).is_empty());

        fs::remove_file(&created).unwrap();
        index.update_affected([event(&created, FileChangeType::DELETED)]);
        assert!(index.analyses(&first_document).is_empty());
    }

    #[test]
    fn changes_roots_without_rebuilding_retained_roots() {
        let first = tempdir().unwrap();
        let retained = tempdir().unwrap();
        let added = tempdir().unwrap();
        for root in [&first, &retained, &added] {
            fs::create_dir_all(root.path().join("templates")).unwrap();
        }
        fs::write(retained.path().join("templates/original.html"), "original").unwrap();
        fs::write(added.path().join("templates/added.html"), "added").unwrap();
        let mut index = WorkspaceIndex::default();
        index.set_roots([file_uri(first.path()), file_uri(retained.path())]);

        fs::write(
            retained.path().join("templates/not-watched.html"),
            "not indexed",
        )
        .unwrap();
        index.change_roots([file_uri(added.path())], [file_uri(first.path())]);

        let retained_document = file_uri(&retained.path().join("templates/page.html"));
        assert_eq!(
            index
                .analyses(&retained_document)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            ["original"]
        );
        let added_document = file_uri(&added.path().join("templates/page.html"));
        assert_eq!(index.analyses(&added_document)[0].0, "added");
        assert!(
            index
                .analyses(&file_uri(&first.path().join("templates/page.html")))
                .is_empty()
        );
    }

    #[test]
    fn ordinary_updates_are_incremental_and_hidden_or_temporary_paths_are_ignored() {
        let workspace = tempdir().unwrap();
        let templates = workspace.path().join("templates");
        fs::create_dir_all(templates.join(".hidden")).unwrap();
        fs::write(templates.join("kept.html"), "{@String kept}").unwrap();
        fs::write(templates.join("untouched.html"), "{@String before}").unwrap();
        fs::write(templates.join(".hidden/secret.html"), "secret").unwrap();
        fs::write(templates.join("backup.html~"), "backup").unwrap();
        fs::write(templates.join("swap.html.swp"), "swap").unwrap();
        let document = file_uri(&templates.join("page.html"));
        let mut index = WorkspaceIndex::default();
        index.set_roots([file_uri(workspace.path())]);
        assert_eq!(
            index
                .analyses(&document)
                .into_iter()
                .map(|(id, _)| id)
                .collect::<Vec<_>>(),
            ["kept", "untouched"]
        );

        fs::write(templates.join("untouched.html"), "{@String after}").unwrap();
        fs::write(templates.join("kept.html"), "{@String changed}").unwrap();
        index.update_affected([event(&templates.join("kept.html"), FileChangeType::CHANGED)]);

        let analyses = index.analyses(&document);
        assert!(analyses.iter().any(|(id, analysis)| {
            *id == "kept" && analysis.template.source.contains("changed")
        }));
        assert!(analyses.iter().any(|(id, analysis)| {
            *id == "untouched" && analysis.template.source.contains("before")
        }));

        index.update_affected([event(&templates, FileChangeType::CHANGED)]);
        assert!(index.analyses(&document).iter().any(|(id, analysis)| {
            *id == "untouched" && analysis.template.source.contains("after")
        }));

        let hidden = templates.join(".new.html");
        fs::write(&hidden, "hidden").unwrap();
        let colliding_backup = templates.join("kept.bak");
        fs::write(&colliding_backup, "backup").unwrap();
        index.update_affected([
            event(&hidden, FileChangeType::CREATED),
            event(&colliding_backup, FileChangeType::CREATED),
        ]);
        assert_eq!(index.analyses(&document).len(), 2);
        assert!(index.valid_id("kept"));
    }
}
