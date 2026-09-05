use std::{
    collections::BTreeMap,
    fs,
    path::{Component, Path, PathBuf},
};

use tower_lsp::lsp_types::{Location, Position, Range, Url};

use radiant_compiler::BUILT_IN_SECTIONS;

#[derive(Debug)]
struct TemplateRoot {
    workspace: PathBuf,
    templates: PathBuf,
    files: BTreeMap<String, PathBuf>,
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
}

/// The templates discovered in each configured workspace folder.
#[derive(Debug, Default)]
pub(crate) struct WorkspaceIndex {
    roots: Vec<TemplateRoot>,
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

    pub(crate) fn template_ids(&self, document: &Url) -> Vec<String> {
        self.root_for_document(document)
            .map(|root| root.files.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub(crate) fn tag_names(&self, document: &Url) -> Vec<String> {
        self.root_for_document(document)
            .map(|root| {
                root.files
                    .keys()
                    .filter_map(|id| id.strip_prefix("tags/").map(ToOwned::to_owned))
                    .filter(|name| !BUILT_IN_SECTIONS.contains(&name.as_str()))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub(crate) fn location(&self, document: &Url, id: &str) -> Option<Location> {
        if !safe_id(id) {
            return None;
        }
        let path = self.root_for_document(document)?.files.get(id)?;
        Some(Location::new(
            Url::from_file_path(path).ok()?,
            Range::new(Position::new(0, 0), Position::new(0, 0)),
        ))
    }

    pub(crate) fn refresh_affected(&mut self, changed: impl IntoIterator<Item = Url>) {
        let paths = changed
            .into_iter()
            .filter_map(|uri| uri.to_file_path().ok())
            .collect::<Vec<_>>();
        for root in &mut self.roots {
            if paths.iter().any(|path| path.starts_with(&root.templates)) {
                root.refresh();
            }
        }
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

fn discover(root: &Path) -> BTreeMap<String, PathBuf> {
    fn visit(directory: &Path, files: &mut Vec<PathBuf>) {
        let Ok(entries) = fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            if file_type.is_dir() {
                visit(&entry.path(), files);
            } else if file_type.is_file() {
                files.push(entry.path());
            }
        }
    }

    let mut paths = Vec::new();
    visit(root, &mut paths);
    paths.sort();
    paths
        .into_iter()
        .filter_map(|path| {
            let relative = path.strip_prefix(root).ok()?.with_extension("");
            let id = relative
                .components()
                .map(|component| component.as_os_str().to_str())
                .collect::<Option<Vec<_>>>()?
                .join("/");
            Some((id, path))
        })
        .collect()
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
    use tower_lsp::lsp_types::Url;

    use super::WorkspaceIndex;

    fn file_uri(path: &std::path::Path) -> Url {
        Url::from_file_path(path).unwrap()
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
            index.template_ids(&document),
            ["nested/card", "tags/admin/badge"]
        );
        assert_eq!(index.tag_names(&document), ["admin/badge"]);
        assert!(index.location(&document, "nested/card").is_some());
        assert!(index.location(&document, "other").is_none());
        assert!(index.location(&document, "../outside").is_none());
        assert!(index.location(&document, "/outside").is_none());
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
        index.refresh_affected([file_uri(&created)]);
        assert_eq!(index.template_ids(&first_document), ["new"]);
        assert!(index.template_ids(&second_document).is_empty());

        fs::remove_file(&created).unwrap();
        index.refresh_affected([file_uri(&created)]);
        assert!(index.template_ids(&first_document).is_empty());
    }
}
