use std::{
    collections::{BTreeSet, HashSet},
    env, fs,
    path::{Component, Path, PathBuf},
};

use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::quote;
use syn::{
    Attribute, Data, DeriveInput, Error, Fields, LitStr, Result, parse_macro_input,
    spanned::Spanned,
};

#[proc_macro_derive(Template, attributes(template))]
pub fn derive_template(input: TokenStream) -> TokenStream {
    expand_template(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

#[proc_macro_derive(TemplateValue, attributes(template_value))]
pub fn derive_template_value(input: TokenStream) -> TokenStream {
    expand_template_value(parse_macro_input!(input as DeriveInput))
        .unwrap_or_else(Error::into_compile_error)
        .into()
}

fn named_fields(
    input: &DeriveInput,
) -> Result<&syn::punctuated::Punctuated<syn::Field, syn::token::Comma>> {
    match &input.data {
        Data::Struct(data) => match &data.fields {
            Fields::Named(fields) => Ok(&fields.named),
            _ => Err(Error::new(
                data.fields.span(),
                "this derive requires a struct with named fields",
            )),
        },
        _ => Err(Error::new(
            input.span(),
            "this derive requires a struct with named fields",
        )),
    }
}

#[derive(Default)]
struct TemplateArgs {
    path: Option<LitStr>,
    root: Option<LitStr>,
}

fn template_args(attrs: &[Attribute]) -> Result<TemplateArgs> {
    let mut result = TemplateArgs::default();
    for attr in attrs.iter().filter(|a| a.path().is_ident("template")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("path") {
                set_once(
                    &mut result.path,
                    meta.value()?.parse()?,
                    meta.path.span(),
                    "path",
                )
            } else if meta.path.is_ident("root") {
                set_once(
                    &mut result.root,
                    meta.value()?.parse()?,
                    meta.path.span(),
                    "root",
                )
            } else {
                Err(meta.error("expected `path` or `root`"))
            }
        })?;
    }
    Ok(result)
}

fn set_once<T>(slot: &mut Option<T>, value: T, span: Span, name: &str) -> Result<()> {
    if slot.is_some() {
        Err(Error::new(span, format!("duplicate `{name}` attribute")))
    } else {
        *slot = Some(value);
        Ok(())
    }
}

enum FieldAttr {
    Keep(Option<String>),
    Skip,
}

fn field_attr(attrs: &[Attribute], attr_name: &str) -> Result<FieldAttr> {
    let mut rename: Option<LitStr> = None;
    let mut skip: Option<Span> = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident(attr_name)) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename") {
                set_once(
                    &mut rename,
                    meta.value()?.parse()?,
                    meta.path.span(),
                    "rename",
                )
            } else if meta.path.is_ident("skip") {
                if !meta.input.is_empty() {
                    return Err(meta.error("`skip` does not take a value"));
                }
                set_once(&mut skip, meta.path.span(), meta.path.span(), "skip")
            } else {
                Err(meta.error("expected `rename` or `skip`"))
            }
        })?;
    }
    if rename.is_some()
        && let Some(span) = skip
    {
        return Err(Error::new(span, "`rename` and `skip` cannot be combined"));
    }
    Ok(if skip.is_some() {
        FieldAttr::Skip
    } else {
        FieldAttr::Keep(rename.map(|s| s.value()))
    })
}

fn map_entries(
    input: &DeriveInput,
    attr: &str,
    rename_all: Option<RenameRule>,
) -> Result<Vec<proc_macro2::TokenStream>> {
    let mut entries = Vec::new();
    let mut names = BTreeSet::new();
    for field in named_fields(input)? {
        let Some(ident) = field.ident.as_ref() else {
            return Err(Error::new(field.span(), "expected a named field"));
        };
        let FieldAttr::Keep(rename) = field_attr(&field.attrs, attr)? else {
            continue;
        };
        let name = rename.unwrap_or_else(|| {
            rename_all.map_or_else(|| ident.to_string(), |r| r.apply(&ident.to_string()))
        });
        if !names.insert(name.clone()) {
            return Err(Error::new(
                field.span(),
                format!("duplicate output field name `{name}`"),
            ));
        }
        entries.push(quote! {
            __radiant_map.insert(::std::string::String::from(#name), ::radiant::IntoValue::into_value(&self.#ident));
        });
    }
    Ok(entries)
}

fn expand_template(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let args = template_args(&input.attrs)?;
    let path_lit = args
        .path
        .ok_or_else(|| Error::new(input.span(), "missing `#[template(path = \"...\")]`"))?;
    let root = args
        .root
        .map_or_else(|| "templates".to_owned(), |s| s.value());
    if Path::new(&root).is_absolute() {
        return Err(Error::new(input.span(), "template root must be relative"));
    }
    let manifest = env::var_os("CARGO_MANIFEST_DIR")
        .ok_or_else(|| Error::new(input.span(), "CARGO_MANIFEST_DIR is not set"))?;
    let root_path = PathBuf::from(manifest).join(root);
    let mut loader = Loader::new(root_path);
    let id = normalize_id(&path_lit.value()).map_err(|m| Error::new(path_lit.span(), m))?;
    loader.load(&id, &mut Vec::new());
    if !loader.errors.is_empty() {
        let mut error = loader.errors.remove(0);
        for other in loader.errors {
            error.combine(other);
        }
        return Err(error);
    }
    let entries = map_entries(&input, "template", None)?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    let ids: Vec<LitStr> = loader
        .sources
        .iter()
        .map(|s| LitStr::new(&s.id, Span::call_site()))
        .collect();
    let sources: Vec<LitStr> = loader
        .sources
        .iter()
        .map(|s| LitStr::new(&s.source, Span::call_site()))
        .collect();
    let paths: Vec<LitStr> = loader
        .sources
        .iter()
        .map(|s| LitStr::new(&s.path.to_string_lossy(), Span::call_site()))
        .collect();
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::radiant::Template for #name #ty_generics #where_clause {
            const ID: &'static str = #id;
            fn data(&self) -> ::radiant::Value {
                let mut __radiant_map = ::std::collections::BTreeMap::new();
                #(#entries)*
                ::radiant::Value::Map(__radiant_map)
            }
            fn sources() -> &'static [::radiant::EmbeddedSource] {
                #(const _: &[u8] = include_bytes!(#paths);)*
                &[#(::radiant::EmbeddedSource { id: #ids, source: #sources }),*]
            }
        }
    })
}

struct Source {
    id: String,
    source: String,
    path: PathBuf,
}
struct Loader {
    root: PathBuf,
    loaded: HashSet<String>,
    sources: Vec<Source>,
    errors: Vec<Error>,
}

impl Loader {
    fn new(root: PathBuf) -> Self {
        Self {
            root,
            loaded: HashSet::new(),
            sources: Vec::new(),
            errors: Vec::new(),
        }
    }

    fn load(&mut self, requested: &str, stack: &mut Vec<String>) {
        let Ok(id) = normalize_id(requested) else {
            self.errors.push(Error::new(
                Span::call_site(),
                format!("invalid template include `{requested}`"),
            ));
            return;
        };
        let Some((resolved_id, path)) = self.resolve(&id) else {
            self.errors.push(Error::new(
                Span::call_site(),
                format!(
                    "template `{id}` was not found under `{}`",
                    self.root.display()
                ),
            ));
            return;
        };
        if let Some(at) = stack.iter().position(|item| item == &resolved_id) {
            let mut cycle = stack[at..].to_vec();
            cycle.push(resolved_id);
            self.errors.push(Error::new(
                Span::call_site(),
                format!("template include cycle: {}", cycle.join(" -> ")),
            ));
            return;
        }
        if self.loaded.contains(&resolved_id) {
            return;
        }
        let source = match fs::read_to_string(&path) {
            Ok(source) => source,
            Err(error) => {
                self.errors.push(Error::new(
                    Span::call_site(),
                    format!(
                        "cannot read template `{}` as UTF-8: {error}",
                        path.display()
                    ),
                ));
                return;
            }
        };
        let template = match radiant_compiler::parse(&resolved_id, &source) {
            Ok(template) => template,
            Err(diagnostics) => {
                self.errors.extend(
                    diagnostics
                        .into_iter()
                        .map(|d| Error::new(Span::call_site(), d.to_string())),
                );
                return;
            }
        };
        self.loaded.insert(resolved_id.clone());
        self.sources.push(Source {
            id: resolved_id.clone(),
            source,
            path,
        });
        stack.push(resolved_id);
        for dependency in template.dependencies() {
            let template_id = dependency
                .split_once('$')
                .map_or(dependency, |(template_id, _)| template_id);
            self.load(template_id, stack);
        }
        stack.pop();
    }

    fn resolve(&self, id: &str) -> Option<(String, PathBuf)> {
        let path = Path::new(id);
        let candidates: Vec<PathBuf> = if path.extension().is_some() {
            vec![path.to_owned()]
        } else {
            ["", ".html", ".txt", ".json"]
                .iter()
                .map(|suffix| PathBuf::from(format!("{id}{suffix}")))
                .collect()
        };
        candidates.into_iter().find_map(|relative| {
            let path = self.root.join(&relative);
            path.is_file()
                .then(|| (relative.to_string_lossy().replace('\\', "/"), path))
        })
    }
}

fn normalize_id(value: &str) -> std::result::Result<String, &'static str> {
    let path = Path::new(value);
    if path.is_absolute() {
        return Err("template paths must be relative to the template root");
    }
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err("template paths may not escape the template root");
            }
        }
    }
    if parts.is_empty() {
        Err("template path may not be empty")
    } else {
        Ok(parts.join("/"))
    }
}

#[derive(Clone, Copy)]
enum RenameRule {
    Camel,
    Snake,
    Kebab,
}

impl RenameRule {
    fn apply(self, value: &str) -> String {
        let words: Vec<&str> = value.split('_').filter(|word| !word.is_empty()).collect();
        match self {
            Self::Snake => words.join("_"),
            Self::Kebab => words.join("-"),
            Self::Camel => words.first().map_or_else(String::new, |first| {
                let mut result = (*first).to_owned();
                for word in &words[1..] {
                    let mut chars = word.chars();
                    if let Some(first) = chars.next() {
                        result.extend(first.to_uppercase());
                        result.extend(chars);
                    }
                }
                result
            }),
        }
    }
}

fn rename_rule(attrs: &[Attribute]) -> Result<Option<RenameRule>> {
    let mut value: Option<LitStr> = None;
    for attr in attrs.iter().filter(|a| a.path().is_ident("template_value")) {
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("rename_all") {
                set_once(
                    &mut value,
                    meta.value()?.parse()?,
                    meta.path.span(),
                    "rename_all",
                )
            } else {
                Err(meta.error("expected `rename_all`"))
            }
        })?;
    }
    value
        .map(|value| match value.value().as_str() {
            "camelCase" => Ok(RenameRule::Camel),
            "snake_case" => Ok(RenameRule::Snake),
            "kebab-case" => Ok(RenameRule::Kebab),
            _ => Err(Error::new(
                value.span(),
                "expected `camelCase`, `snake_case`, or `kebab-case`",
            )),
        })
        .transpose()
}

fn expand_template_value(input: DeriveInput) -> Result<proc_macro2::TokenStream> {
    let entries = map_entries(&input, "template_value", rename_rule(&input.attrs)?)?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();
    Ok(quote! {
        #[automatically_derived]
        impl #impl_generics ::radiant::TemplateValue for #name #ty_generics #where_clause {
            fn to_value(&self) -> ::radiant::Value {
                let mut __radiant_map = ::std::collections::BTreeMap::new();
                #(#entries)*
                ::radiant::Value::Map(__radiant_map)
            }
        }
        #[automatically_derived]
        impl #impl_generics ::radiant::IntoValue for &#name #ty_generics #where_clause {
            fn into_value(self) -> ::radiant::Value { ::radiant::TemplateValue::to_value(self) }
        }
        #[automatically_derived]
        impl #impl_generics ::radiant::IntoValue for #name #ty_generics #where_clause {
            fn into_value(self) -> ::radiant::Value { ::radiant::TemplateValue::to_value(&self) }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_id("./a/b.html"), Ok("a/b.html".into()));
        assert!(normalize_id("../x").is_err());
    }
    #[test]
    fn applies_rename_rules() {
        assert_eq!(RenameRule::Camel.apply("first_name"), "firstName");
        assert_eq!(RenameRule::Kebab.apply("first_name"), "first-name");
    }
}
