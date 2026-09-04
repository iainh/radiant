use std::{
    collections::{BTreeMap, HashMap},
    error::Error,
    future::Future,
    path::{Component, Path, PathBuf},
    pin::Pin,
    sync::{Arc, RwLock},
};

use futures_util::FutureExt;
use radiant_compiler::{Argument, ArgumentValue, BinaryOp, Expr, Literal, Node, Section, UnaryOp};

use crate::{
    DynamicTemplate, EmbeddedSource, ErrorCode, MediaType, RenderError, Rendered, Template, Value,
    escape,
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Debug, Clone, PartialEq)]
pub enum Resolution<T> {
    Value(T),
    NotFound,
}

pub struct EvalContext<'a> {
    pub base: &'a Value,
    pub name: &'a str,
    pub arguments: &'a [Value],
}

pub struct NamespaceContext<'a> {
    pub name: &'a str,
    pub arguments: &'a [Value],
    pub language: Option<&'a str>,
}

pub trait ValueResolver: Send + Sync {
    fn priority(&self) -> i32 {
        0
    }

    fn resolve<'a>(
        &'a self,
        context: EvalContext<'a>,
    ) -> BoxFuture<'a, Result<Resolution<Value>, RenderError>>;
}

pub trait NamespaceResolver: Send + Sync {
    fn namespace(&self) -> &str;

    fn priority(&self) -> i32 {
        0
    }

    fn resolve<'a>(
        &'a self,
        context: NamespaceContext<'a>,
    ) -> BoxFuture<'a, Result<Resolution<Value>, RenderError>>;
}

pub trait TemplateLoader: Send + Sync {
    fn load(&self, id: &str) -> Result<Option<String>, Box<dyn Error + Send + Sync>>;
}

#[derive(Debug, Clone)]
pub struct FileLoader {
    root: PathBuf,
}

impl FileLoader {
    #[must_use]
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }
}

impl TemplateLoader for FileLoader {
    fn load(&self, id: &str) -> Result<Option<String>, Box<dyn Error + Send + Sync>> {
        let path = Path::new(id);
        if path.is_absolute()
            || path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Ok(None);
        }
        let path = self.root.join(path);
        match std::fs::read_to_string(path) {
            Ok(source) => Ok(Some(source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(Box::new(error)),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderOptions {
    pub media_type: Option<MediaType>,
    pub language: Option<String>,
}

#[derive(Clone)]
pub struct Engine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    templates: RwLock<HashMap<String, Arc<radiant_compiler::Template>>>,
    resolvers: Vec<Arc<dyn ValueResolver>>,
    namespaces: Vec<Arc<dyn NamespaceResolver>>,
    loaders: Vec<Arc<dyn TemplateLoader>>,
    strict: bool,
    max_include_depth: usize,
    max_output_bytes: usize,
    allowed_sections: Option<Vec<String>>,
    allowed_namespaces: Option<Vec<String>>,
}

pub struct EngineBuilder {
    templates: Vec<(String, String)>,
    resolvers: Vec<Arc<dyn ValueResolver>>,
    namespaces: Vec<Arc<dyn NamespaceResolver>>,
    loaders: Vec<Arc<dyn TemplateLoader>>,
    strict: bool,
    max_include_depth: usize,
    max_output_bytes: usize,
    allowed_sections: Option<Vec<String>>,
    allowed_namespaces: Option<Vec<String>>,
}

impl Default for EngineBuilder {
    fn default() -> Self {
        Self {
            templates: Vec::new(),
            resolvers: Vec::new(),
            namespaces: Vec::new(),
            loaders: Vec::new(),
            strict: true,
            max_include_depth: 64,
            max_output_bytes: 8 * 1024 * 1024,
            allowed_sections: None,
            allowed_namespaces: None,
        }
    }
}

impl EngineBuilder {
    #[must_use]
    pub fn template(mut self, id: impl Into<String>, source: impl Into<String>) -> Self {
        self.templates.push((id.into(), source.into()));
        self
    }

    #[must_use]
    pub fn loader(mut self, loader: impl TemplateLoader + 'static) -> Self {
        self.loaders.push(Arc::new(loader));
        self
    }

    #[must_use]
    pub fn value_resolver(mut self, resolver: impl ValueResolver + 'static) -> Self {
        self.resolvers.push(Arc::new(resolver));
        self
    }

    #[must_use]
    pub fn namespace_resolver(mut self, resolver: impl NamespaceResolver + 'static) -> Self {
        self.namespaces.push(Arc::new(resolver));
        self
    }

    #[must_use]
    pub const fn strict(mut self, strict: bool) -> Self {
        self.strict = strict;
        self
    }

    #[must_use]
    pub const fn max_include_depth(mut self, maximum: usize) -> Self {
        self.max_include_depth = maximum;
        self
    }

    #[must_use]
    pub const fn max_output_bytes(mut self, maximum: usize) -> Self {
        self.max_output_bytes = maximum;
        self
    }

    /// Applies conservative limits and a minimal section/namespace allowlist
    /// for user-supplied templates.
    #[must_use]
    pub fn restricted(mut self) -> Self {
        self.allowed_sections = Some(
            ["if", "for", "each", "let", "set", "when", "switch"]
                .into_iter()
                .map(str::to_owned)
                .collect(),
        );
        self.allowed_namespaces = Some(vec!["data".into()]);
        self.max_include_depth = 0;
        self.max_output_bytes = 1024 * 1024;
        self
    }

    #[must_use]
    pub fn allow_section(mut self, name: impl Into<String>) -> Self {
        self.allowed_sections
            .get_or_insert_with(Vec::new)
            .push(name.into());
        self
    }

    #[must_use]
    pub fn allow_namespace(mut self, name: impl Into<String>) -> Self {
        self.allowed_namespaces
            .get_or_insert_with(Vec::new)
            .push(name.into());
        self
    }

    pub fn build(mut self) -> Result<Engine, RenderError> {
        self.resolvers
            .sort_by_key(|resolver| std::cmp::Reverse(resolver.priority()));
        self.namespaces
            .sort_by_key(|resolver| std::cmp::Reverse(resolver.priority()));
        let engine = Engine {
            inner: Arc::new(EngineInner {
                templates: RwLock::new(HashMap::new()),
                resolvers: self.resolvers,
                namespaces: self.namespaces,
                loaders: self.loaders,
                strict: self.strict,
                max_include_depth: self.max_include_depth,
                max_output_bytes: self.max_output_bytes,
                allowed_sections: self.allowed_sections,
                allowed_namespaces: self.allowed_namespaces,
            }),
        };
        for (id, source) in self.templates {
            engine.register(id, source)?;
        }
        Ok(engine)
    }
}

impl Engine {
    pub fn new() -> Result<Self, RenderError> {
        Self::builder().build()
    }

    #[must_use]
    pub fn builder() -> EngineBuilder {
        EngineBuilder::default()
    }

    pub fn register(
        &self,
        id: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<(), RenderError> {
        let id = normalize_id(&id.into())?;
        let source = source.into();
        let parsed = Arc::new(radiant_compiler::parse(&id, &source).map_err(RenderError::from)?);
        self.validate_policy(&parsed)?;
        let mut templates = self
            .inner
            .templates
            .write()
            .unwrap_or_else(|e| e.into_inner());
        if templates.contains_key(&id) {
            return Err(RenderError::new(
                ErrorCode::DuplicateTemplate,
                format!("template `{id}` is already registered"),
            ));
        }
        templates.insert(id, parsed);
        Ok(())
    }

    pub fn replace(
        &self,
        id: impl Into<String>,
        source: impl Into<String>,
    ) -> Result<(), RenderError> {
        let id = normalize_id(&id.into())?;
        let source = source.into();
        let parsed = Arc::new(radiant_compiler::parse(&id, &source).map_err(RenderError::from)?);
        self.validate_policy(&parsed)?;
        self.inner
            .templates
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(id, parsed);
        Ok(())
    }

    pub async fn reload(&self, id: &str) -> Result<(), RenderError> {
        let id = normalize_id(id)?;
        for loader in &self.inner.loaders {
            match loader.load(&id) {
                Ok(Some(source)) => return self.replace(id, source),
                Ok(None) => {}
                Err(error) => {
                    return Err(RenderError::new(
                        ErrorCode::Loader,
                        format!("could not reload template `{id}`"),
                    )
                    .with_source(error));
                }
            }
        }
        Err(missing_template(&id))
    }

    pub async fn template(&self, id: &str) -> Result<DynamicTemplate, RenderError> {
        let requested = normalize_id(id)?;
        self.resolve_template_id(&requested, None, None).await?;
        Ok(DynamicTemplate {
            engine: self.clone(),
            id: requested,
        })
    }

    pub async fn render<T: Template>(&self, template: T) -> Result<Rendered, RenderError> {
        self.render_with(template).render().await
    }

    #[must_use]
    pub fn render_with<T: Template>(&self, template: T) -> RenderBuilder<T> {
        RenderBuilder {
            engine: self.clone(),
            template,
            options: RenderOptions::default(),
        }
    }

    pub(crate) async fn render_dynamic(
        &self,
        id: &str,
        data: Value,
        media_type: Option<MediaType>,
        language: Option<String>,
    ) -> Result<Rendered, RenderError> {
        self.render_value(
            id,
            data,
            RenderOptions {
                media_type,
                language,
            },
        )
        .await
    }

    async fn render_value(
        &self,
        id: &str,
        data: Value,
        options: RenderOptions,
    ) -> Result<Rendered, RenderError> {
        let (base_id, fragment) = id
            .rsplit_once('$')
            .map_or((id, None), |(base, fragment)| (base, Some(fragment)));
        let id = self
            .resolve_template_id(base_id, options.media_type, options.language.as_deref())
            .await?;
        let template = self.compiled(&id).ok_or_else(|| missing_template(&id))?;
        let media_type = options
            .media_type
            .unwrap_or_else(|| MediaType::from_template_id(&id));
        let mut state = RenderState {
            scopes: vec![data],
            root: 0,
            include_stack: vec![id.clone()],
            overrides: Vec::new(),
            media_type,
            language: options.language.clone(),
        };
        let mut output = String::new();
        if let Some(fragment) = fragment {
            let section = template
                .fragments()
                .into_iter()
                .find(|section| {
                    section.arguments.first().and_then(Argument::static_text) == Some(fragment)
                })
                .ok_or_else(|| {
                    RenderError::new(
                        ErrorCode::MissingTemplate,
                        format!("fragment `{fragment}` not found in `{id}`"),
                    )
                })?;
            if let Some(block) = section.blocks.first() {
                self.render_nodes(&block.nodes, &template, &mut state, &mut output)
                    .await?;
            }
        } else {
            self.render_nodes(&template.nodes, &template, &mut state, &mut output)
                .await?;
        }
        Ok(Rendered::new(output, media_type, options.language))
    }

    fn render_nodes<'a>(
        &'a self,
        nodes: &'a [Node],
        template: &'a radiant_compiler::Template,
        state: &'a mut RenderState,
        output: &'a mut String,
    ) -> BoxFuture<'a, Result<(), RenderError>> {
        async move {
            for node in nodes {
                match node {
                    Node::Text { value, .. } | Node::Unparsed { value, .. } => {
                        output.push_str(value);
                    }
                    Node::Comment { .. } | Node::Parameter(_) => {}
                    Node::Output { expression, span } => {
                        match self.evaluate(expression, template, state).await? {
                            Resolution::Value(value) => {
                                write_value(&value, state.media_type, output)
                                    .map_err(|error| error.at(template, *span))?;
                            }
                            Resolution::NotFound if self.inner.strict => {
                                return Err(RenderError::new(
                                    ErrorCode::MissingValue,
                                    "expression could not be resolved",
                                )
                                .at(template, *span));
                            }
                            Resolution::NotFound => {}
                        }
                    }
                    Node::Section(section) => {
                        self.render_section(section, template, state, output)
                            .await?;
                    }
                }
                if output.len() > self.inner.max_output_bytes {
                    return Err(RenderError::new(
                        ErrorCode::OutputLimit,
                        format!(
                            "rendered output exceeded {} bytes",
                            self.inner.max_output_bytes
                        ),
                    )
                    .at(template, node.span()));
                }
            }
            Ok(())
        }
        .boxed()
    }

    fn render_section<'a>(
        &'a self,
        section: &'a Section,
        template: &'a radiant_compiler::Template,
        state: &'a mut RenderState,
        output: &'a mut String,
    ) -> BoxFuture<'a, Result<(), RenderError>> {
        async move {
            match section.name.as_str() {
                "if" => {
                    let condition = self
                        .argument(section.arguments.first(), template, state)
                        .await?;
                    let block = if condition.is_truthy() {
                        section.blocks.first()
                    } else {
                        section.blocks.iter().find(|block| block.name == "else")
                    };
                    if let Some(block) = block {
                        self.render_nodes(&block.nodes, template, state, output)
                            .await?;
                    }
                }
                "for" | "each" => {
                    self.render_loop(section, template, state, output).await?;
                }
                "let" | "set" => {
                    let values = self
                        .named_arguments(&section.arguments, template, state)
                        .await?;
                    state.scopes.push(Value::Map(values));
                    if let Some(block) = section.blocks.first() {
                        self.render_nodes(&block.nodes, template, state, output)
                            .await?;
                    }
                    state.scopes.pop();
                }
                "when" | "switch" => {
                    self.render_when(section, template, state, output).await?;
                }
                "include" => {
                    self.render_include(section, template, state, output, false)
                        .await?;
                }
                "insert" => {
                    let name = section
                        .arguments
                        .first()
                        .and_then(Argument::static_text)
                        .unwrap_or("");
                    let override_nodes = state
                        .overrides
                        .last()
                        .and_then(|overrides| overrides.get(name))
                        .cloned();
                    if let Some(nodes) = override_nodes {
                        self.render_nodes(&nodes, template, state, output).await?;
                    } else if let Some(block) = section.blocks.first() {
                        self.render_nodes(&block.nodes, template, state, output)
                            .await?;
                    }
                }
                "fragment" => {
                    let hidden = section.arguments.iter().any(|argument| {
                        argument.name.as_deref() == Some("_hidden")
                            || (argument.name.as_deref() == Some("rendered")
                                && matches!(
                                    argument.value,
                                    ArgumentValue::Expression(Expr::Literal {
                                        value: Literal::Bool(false),
                                        ..
                                    })
                                ))
                    });
                    if !hidden && let Some(block) = section.blocks.first() {
                        self.render_nodes(&block.nodes, template, state, output)
                            .await?;
                    }
                }
                "capture" => {}
                _ => {
                    self.render_include(section, template, state, output, true)
                        .await?;
                }
            }
            Ok(())
        }
        .boxed()
    }

    async fn render_loop(
        &self,
        section: &Section,
        template: &radiant_compiler::Template,
        state: &mut RenderState,
        output: &mut String,
    ) -> Result<(), RenderError> {
        let alias = section
            .arguments
            .iter()
            .find(|argument| argument.name.as_deref() == Some("alias"))
            .and_then(Argument::static_text)
            .unwrap_or("it")
            .to_owned();
        let source_argument = section
            .arguments
            .iter()
            .find(|argument| argument.name.as_deref() == Some("in"))
            .or_else(|| section.arguments.first());
        let source = self.argument(source_argument, template, state).await?;
        let values = match source {
            Value::Sequence(values) => values,
            Value::Map(values) => values
                .into_iter()
                .map(|(key, value)| {
                    Value::Map(BTreeMap::from([
                        ("key".into(), Value::String(key)),
                        ("value".into(), value),
                    ]))
                })
                .collect(),
            Value::Null => Vec::new(),
            other => {
                return Err(RenderError::new(
                    ErrorCode::Type,
                    format!("cannot iterate over {}", other.type_name()),
                )
                .at(template, section.span));
            }
        };
        if values.is_empty() {
            if let Some(block) = section.blocks.iter().find(|block| block.name == "else") {
                self.render_nodes(&block.nodes, template, state, output)
                    .await?;
            }
            return Ok(());
        }
        let length = values.len();
        for (index, value) in values.into_iter().enumerate() {
            let mut scope = BTreeMap::new();
            scope.insert(alias.clone(), value);
            scope.insert(format!("{alias}_index"), Value::Integer(index as i64));
            scope.insert(format!("{alias}_count"), Value::Integer((index + 1) as i64));
            scope.insert(format!("{alias}_isFirst"), Value::Bool(index == 0));
            scope.insert(format!("{alias}_isLast"), Value::Bool(index + 1 == length));
            scope.insert(format!("{alias}_hasNext"), Value::Bool(index + 1 < length));
            state.scopes.push(Value::Map(scope));
            if let Some(block) = section.blocks.first() {
                self.render_nodes(&block.nodes, template, state, output)
                    .await?;
            }
            state.scopes.pop();
        }
        Ok(())
    }

    async fn render_when(
        &self,
        section: &Section,
        template: &radiant_compiler::Template,
        state: &mut RenderState,
        output: &mut String,
    ) -> Result<(), RenderError> {
        let tested = self
            .argument(section.arguments.first(), template, state)
            .await?;
        for block in section.blocks.iter().skip(1) {
            if block.name == "else" {
                self.render_nodes(&block.nodes, template, state, output)
                    .await?;
                return Ok(());
            }
            if let Some(argument) = block.arguments.first() {
                let candidate = self.argument(Some(argument), template, state).await?;
                if values_equal(&tested, &candidate) {
                    self.render_nodes(&block.nodes, template, state, output)
                        .await?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn render_include(
        &self,
        section: &Section,
        template: &radiant_compiler::Template,
        state: &mut RenderState,
        output: &mut String,
        tag: bool,
    ) -> Result<(), RenderError> {
        if state.include_stack.len() >= self.inner.max_include_depth {
            return Err(RenderError::new(
                ErrorCode::IncludeCycle,
                "maximum include depth exceeded",
            )
            .at(template, section.span));
        }
        let mut id = if tag {
            format!("tags/{}", section.name)
        } else if let Some(dynamic) = section
            .arguments
            .iter()
            .find(|argument| argument.name.as_deref() == Some("_id"))
        {
            self.argument(Some(dynamic), template, state)
                .await?
                .plain_text()
        } else {
            section
                .arguments
                .first()
                .and_then(Argument::static_text)
                .ok_or_else(|| {
                    RenderError::new(ErrorCode::Type, "include requires a template ID")
                        .at(template, section.span)
                })?
                .to_owned()
        };
        if let Some((template_id, fragment)) = id
            .split_once('$')
            .map(|(template_id, fragment)| (template_id.to_owned(), fragment.to_owned()))
        {
            id = template_id;
            let id = self
                .resolve_template_id(&id, Some(state.media_type), state.language.as_deref())
                .await?;
            let include_key = format!("{id}${fragment}");
            if state.include_stack.contains(&include_key) {
                let mut stack = state.include_stack.clone();
                stack.push(include_key.clone());
                let mut error = RenderError::new(
                    ErrorCode::IncludeCycle,
                    format!("recursive include of fragment `{include_key}`"),
                )
                .at(template, section.span);
                error.render_stack = stack;
                return Err(error);
            }
            let included = self.compiled(&id).ok_or_else(|| missing_template(&id))?;
            let fragment_section = included
                .fragments()
                .into_iter()
                .find(|candidate| {
                    candidate.arguments.first().and_then(Argument::static_text)
                        == Some(fragment.as_str())
                })
                .ok_or_else(|| {
                    RenderError::new(
                        ErrorCode::MissingTemplate,
                        format!("fragment `{fragment}` not found in `{id}`"),
                    )
                })?;
            let params = self.include_parameters(section, template, state).await?;
            state.scopes.push(Value::Map(params));
            state.include_stack.push(include_key);
            let result = if let Some(block) = fragment_section.blocks.first() {
                self.render_nodes(&block.nodes, &included, state, output)
                    .await
            } else {
                Ok(())
            };
            state.include_stack.pop();
            state.scopes.pop();
            return result;
        }
        let id = self
            .resolve_template_id(&id, Some(state.media_type), state.language.as_deref())
            .await?;
        if state.include_stack.contains(&id) {
            let mut stack = state.include_stack.clone();
            stack.push(id.clone());
            let mut error = RenderError::new(
                ErrorCode::IncludeCycle,
                format!("recursive include of `{id}`"),
            )
            .at(template, section.span);
            error.render_stack = stack;
            return Err(error);
        }
        let included = self.compiled(&id).ok_or_else(|| missing_template(&id))?;
        let params = self.include_parameters(section, template, state).await?;
        let isolated = tag
            || section.arguments.iter().any(|argument| {
                argument.name.as_deref() == Some("_isolated")
                    && matches!(
                        argument.value,
                        ArgumentValue::Expression(Expr::Literal {
                            value: Literal::Bool(true),
                            ..
                        })
                    )
            });
        let saved_scopes = isolated.then(|| std::mem::take(&mut state.scopes));
        state.scopes.push(Value::Map(params));
        let mut overrides = HashMap::new();
        if let Some(main) = section.blocks.first()
            && !main.nodes.is_empty()
        {
            overrides.insert(String::new(), main.nodes.clone());
        }
        for block in section.blocks.iter().skip(1) {
            overrides.insert(block.name.clone(), block.nodes.clone());
        }
        state.overrides.push(overrides);
        state.include_stack.push(id.clone());
        let result = self
            .render_nodes(&included.nodes, &included, state, output)
            .await;
        state.include_stack.pop();
        state.overrides.pop();
        state.scopes.pop();
        if let Some(scopes) = saved_scopes {
            state.scopes = scopes;
        }
        result.map_err(|mut error| {
            error.render_stack = state.include_stack.clone();
            error
        })
    }

    async fn include_parameters(
        &self,
        section: &Section,
        template: &radiant_compiler::Template,
        state: &mut RenderState,
    ) -> Result<BTreeMap<String, Value>, RenderError> {
        let mut values = BTreeMap::new();
        for argument in &section.arguments {
            let Some(name) = argument.name.as_deref() else {
                continue;
            };
            if name.starts_with('_') {
                continue;
            }
            values.insert(
                name.to_owned(),
                self.argument(Some(argument), template, state).await?,
            );
        }
        Ok(values)
    }

    async fn named_arguments(
        &self,
        arguments: &[Argument],
        template: &radiant_compiler::Template,
        state: &mut RenderState,
    ) -> Result<BTreeMap<String, Value>, RenderError> {
        let mut values = BTreeMap::new();
        for argument in arguments {
            if let Some(name) = &argument.name {
                values.insert(
                    name.clone(),
                    self.argument(Some(argument), template, state).await?,
                );
            }
        }
        Ok(values)
    }

    async fn argument(
        &self,
        argument: Option<&Argument>,
        template: &radiant_compiler::Template,
        state: &mut RenderState,
    ) -> Result<Value, RenderError> {
        let argument = argument
            .ok_or_else(|| RenderError::new(ErrorCode::Type, "section argument is missing"))?;
        match &argument.value {
            ArgumentValue::String(value) | ArgumentValue::Raw(value) => {
                Ok(Value::String(value.clone()))
            }
            ArgumentValue::Expression(expression) => {
                match self.evaluate(expression, template, state).await? {
                    Resolution::Value(value) => Ok(value),
                    Resolution::NotFound if self.inner.strict => Err(RenderError::new(
                        ErrorCode::MissingValue,
                        "section argument could not be resolved",
                    )
                    .at(template, argument.span)),
                    Resolution::NotFound => Ok(Value::Null),
                }
            }
        }
    }

    fn evaluate<'a>(
        &'a self,
        expression: &'a Expr,
        template: &'a radiant_compiler::Template,
        state: &'a RenderState,
    ) -> BoxFuture<'a, Result<Resolution<Value>, RenderError>> {
        async move {
            let result = match expression {
                Expr::Literal { value, .. } => Resolution::Value(match value {
                    Literal::Null => Value::Null,
                    Literal::Bool(value) => Value::Bool(*value),
                    Literal::String(value) => Value::String(value.clone()),
                    Literal::Integer(value) => Value::Integer(*value),
                    Literal::Float(value) => Value::Float(*value),
                }),
                Expr::Identifier { name, .. } => state.lookup(name),
                Expr::Namespace {
                    namespace, name, ..
                } => self.resolve_namespace(namespace, name, &[], state).await?,
                Expr::Member { object, member, .. } => {
                    match self.evaluate(object, template, state).await? {
                        Resolution::Value(base) => self.resolve_member(&base, member, &[]).await?,
                        Resolution::NotFound => Resolution::NotFound,
                    }
                }
                Expr::Index { object, index, .. } => {
                    let base = self.required(object, template, state).await?;
                    let index = self.required(index, template, state).await?;
                    resolve_index(&base, &index)
                }
                Expr::Call {
                    callee, arguments, ..
                } => {
                    let mut values = Vec::with_capacity(arguments.len());
                    for argument in arguments {
                        values.push(self.required(argument, template, state).await?);
                    }
                    match callee.as_ref() {
                        Expr::Namespace {
                            namespace, name, ..
                        } => {
                            self.resolve_namespace(namespace, name, &values, state)
                                .await?
                        }
                        Expr::Member { object, member, .. } => {
                            let base = self.required(object, template, state).await?;
                            self.resolve_member(&base, member, &values).await?
                        }
                        _ => {
                            return Err(RenderError::new(
                                ErrorCode::Type,
                                "only namespace and member functions can be called",
                            )
                            .at(template, expression.span()));
                        }
                    }
                }
                Expr::Safe { expression, .. } => {
                    match self.evaluate(expression, template, state).await? {
                        Resolution::NotFound => Resolution::Value(Value::Null),
                        value => value,
                    }
                }
                Expr::Unary { op, expression, .. } => {
                    let value = self.required(expression, template, state).await?;
                    Resolution::Value(match op {
                        UnaryOp::Not => Value::Bool(!value.is_truthy()),
                        UnaryOp::Negate => match value {
                            Value::Integer(value) => {
                                Value::Integer(value.checked_neg().ok_or_else(|| {
                                    RenderError::new(ErrorCode::Arithmetic, "integer overflow")
                                })?)
                            }
                            Value::Float(value) => Value::Float(-value),
                            other => {
                                return Err(RenderError::new(
                                    ErrorCode::Type,
                                    format!("cannot negate {}", other.type_name()),
                                )
                                .at(template, expression.span()));
                            }
                        },
                    })
                }
                Expr::Binary {
                    op, left, right, ..
                } => {
                    if *op == BinaryOp::Elvis {
                        match self.evaluate(left, template, state).await? {
                            Resolution::NotFound | Resolution::Value(Value::Null) => {
                                self.evaluate(right, template, state).await?
                            }
                            value => value,
                        }
                    } else if *op == BinaryOp::And {
                        let left = self.required(left, template, state).await?;
                        if left.is_truthy() {
                            Resolution::Value(Value::Bool(
                                self.required(right, template, state).await?.is_truthy(),
                            ))
                        } else {
                            Resolution::Value(Value::Bool(false))
                        }
                    } else if *op == BinaryOp::Or {
                        let left = self.required(left, template, state).await?;
                        if left.is_truthy() {
                            Resolution::Value(Value::Bool(true))
                        } else {
                            Resolution::Value(Value::Bool(
                                self.required(right, template, state).await?.is_truthy(),
                            ))
                        }
                    } else {
                        let left = self.required(left, template, state).await?;
                        let right = self.required(right, template, state).await?;
                        Resolution::Value(
                            binary(*op, left, right)
                                .map_err(|error| error.at(template, expression.span()))?,
                        )
                    }
                }
            };
            Ok(result)
        }
        .boxed()
    }

    async fn required(
        &self,
        expression: &Expr,
        template: &radiant_compiler::Template,
        state: &RenderState,
    ) -> Result<Value, RenderError> {
        match self.evaluate(expression, template, state).await? {
            Resolution::Value(value) => Ok(value),
            Resolution::NotFound => Err(RenderError::new(
                ErrorCode::MissingValue,
                "expression operand could not be resolved",
            )
            .at(template, expression.span())),
        }
    }

    async fn resolve_namespace(
        &self,
        namespace: &str,
        name: &str,
        arguments: &[Value],
        state: &RenderState,
    ) -> Result<Resolution<Value>, RenderError> {
        if namespace == "data" {
            return Ok(state.lookup_root(name));
        }
        if self
            .inner
            .allowed_namespaces
            .as_ref()
            .is_some_and(|allowed| !allowed.iter().any(|value| value == namespace))
        {
            return Err(RenderError::new(
                ErrorCode::Extension,
                format!("namespace `{namespace}` is not allowed"),
            ));
        }
        for resolver in self
            .inner
            .namespaces
            .iter()
            .filter(|resolver| resolver.namespace() == namespace)
        {
            match resolver
                .resolve(NamespaceContext {
                    name,
                    arguments,
                    language: state.language.as_deref(),
                })
                .await?
            {
                Resolution::NotFound => {}
                value => return Ok(value),
            }
        }
        Ok(Resolution::NotFound)
    }

    async fn resolve_member(
        &self,
        base: &Value,
        name: &str,
        arguments: &[Value],
    ) -> Result<Resolution<Value>, RenderError> {
        for resolver in &self.inner.resolvers {
            match resolver
                .resolve(EvalContext {
                    base,
                    name,
                    arguments,
                })
                .await?
            {
                Resolution::NotFound => {}
                value => return Ok(value),
            }
        }
        Ok(resolve_builtin(base, name, arguments))
    }

    async fn resolve_template_id(
        &self,
        requested: &str,
        media_type: Option<MediaType>,
        language: Option<&str>,
    ) -> Result<String, RenderError> {
        let requested = normalize_id(requested)?;
        let mut candidates = Vec::new();
        if Path::new(&requested).extension().is_some() {
            candidates.push(requested.clone());
        } else {
            if let Some(media_type) = media_type {
                if let Some(language) = language {
                    candidates.push(format!("{requested}.{language}{}", media_type.suffix()));
                    if let Some(base_language) = base_language(language) {
                        candidates.push(format!(
                            "{requested}.{base_language}{}",
                            media_type.suffix()
                        ));
                    }
                }
                candidates.push(format!("{}{}", requested, media_type.suffix()));
            } else {
                candidates.push(requested.clone());
                for suffix in [".html", ".txt", ".json", ".xml"] {
                    if let Some(language) = language {
                        candidates.push(format!("{requested}.{language}{suffix}"));
                        if let Some(base_language) = base_language(language) {
                            candidates.push(format!("{requested}.{base_language}{suffix}"));
                        }
                    }
                    candidates.push(format!("{requested}{suffix}"));
                }
            }
        }
        candidates.dedup();
        for candidate in candidates {
            if self.compiled(&candidate).is_some() {
                return Ok(candidate);
            }
            for loader in &self.inner.loaders {
                match loader.load(&candidate) {
                    Ok(Some(source)) => {
                        self.replace(&candidate, source)?;
                        return Ok(candidate);
                    }
                    Ok(None) => {}
                    Err(error) => {
                        return Err(RenderError::new(
                            ErrorCode::Loader,
                            format!("could not load template `{candidate}`"),
                        )
                        .with_source(error));
                    }
                }
            }
        }
        if let Some(media_type) = media_type {
            Err(RenderError::new(
                ErrorCode::NotAcceptable,
                format!(
                    "template `{requested}` has no {} variant",
                    media_type.content_type()
                ),
            ))
        } else {
            Err(missing_template(&requested))
        }
    }

    fn compiled(&self, id: &str) -> Option<Arc<radiant_compiler::Template>> {
        self.inner
            .templates
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(id)
            .cloned()
    }

    fn register_embedded(&self, sources: &[EmbeddedSource]) -> Result<(), RenderError> {
        for source in sources {
            let id = normalize_id(source.id)?;
            if let Some(existing) = self.compiled(&id) {
                if existing.source == source.source {
                    continue;
                }
                return Err(RenderError::new(
                    ErrorCode::DuplicateTemplate,
                    format!("template `{id}` is embedded with conflicting sources"),
                ));
            }
            let parsed =
                Arc::new(radiant_compiler::parse(&id, source.source).map_err(RenderError::from)?);
            self.validate_policy(&parsed)?;
            let mut templates = self
                .inner
                .templates
                .write()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(existing) = templates.get(&id) {
                if existing.source != source.source {
                    return Err(RenderError::new(
                        ErrorCode::DuplicateTemplate,
                        format!("template `{id}` is embedded with conflicting sources"),
                    ));
                }
            } else {
                templates.insert(id, parsed);
            }
        }
        Ok(())
    }

    fn validate_policy(&self, template: &radiant_compiler::Template) -> Result<(), RenderError> {
        fn disallowed(nodes: &[Node], allowed: &[String]) -> Option<String> {
            for node in nodes {
                if let Node::Section(section) = node {
                    if !allowed.iter().any(|name| name == &section.name) {
                        return Some(section.name.clone());
                    }
                    for block in &section.blocks {
                        if let Some(name) = disallowed(&block.nodes, allowed) {
                            return Some(name);
                        }
                    }
                }
            }
            None
        }

        if let Some(allowed) = &self.inner.allowed_sections
            && let Some(section) = disallowed(&template.nodes, allowed)
        {
            return Err(RenderError::new(
                ErrorCode::UnknownSection,
                format!("section `{section}` is not allowed by this engine"),
            ));
        }
        Ok(())
    }
}

pub struct RenderBuilder<T> {
    engine: Engine,
    template: T,
    options: RenderOptions,
}

impl<T: Template> RenderBuilder<T> {
    #[must_use]
    pub fn locale(mut self, language: impl Into<String>) -> Self {
        self.options.language = Some(language.into());
        self
    }

    #[must_use]
    pub const fn variant(mut self, media_type: MediaType) -> Self {
        self.options.media_type = Some(media_type);
        self
    }

    pub async fn render(self) -> Result<Rendered, RenderError> {
        self.engine.register_embedded(T::sources())?;
        self.engine
            .render_value(T::ID, self.template.data(), self.options)
            .await
    }
}

struct RenderState {
    scopes: Vec<Value>,
    root: usize,
    include_stack: Vec<String>,
    overrides: Vec<HashMap<String, Vec<Node>>>,
    media_type: MediaType,
    language: Option<String>,
}

impl RenderState {
    fn lookup(&self, name: &str) -> Resolution<Value> {
        if name == "this" {
            return self
                .scopes
                .last()
                .cloned()
                .map_or(Resolution::NotFound, Resolution::Value);
        }
        for scope in self.scopes.iter().rev() {
            if let Value::Map(values) = scope
                && let Some(value) = values.get(name)
            {
                return Resolution::Value(value.clone());
            }
        }
        Resolution::NotFound
    }

    fn lookup_root(&self, name: &str) -> Resolution<Value> {
        match self.scopes.get(self.root) {
            Some(Value::Map(values)) => values
                .get(name)
                .cloned()
                .map_or(Resolution::NotFound, Resolution::Value),
            _ => Resolution::NotFound,
        }
    }
}

fn resolve_builtin(base: &Value, name: &str, arguments: &[Value]) -> Resolution<Value> {
    let no_arguments = arguments.is_empty();
    match (base, name) {
        (Value::Map(values), key) if no_arguments => values
            .get(key)
            .cloned()
            .map_or(Resolution::NotFound, Resolution::Value),
        (Value::Map(values), "size" | "length") if no_arguments => {
            Resolution::Value(Value::Integer(values.len() as i64))
        }
        (Value::Map(values), "isEmpty" | "empty") if no_arguments => {
            Resolution::Value(Value::Bool(values.is_empty()))
        }
        (Value::Map(values), "keys") if no_arguments => Resolution::Value(Value::Sequence(
            values.keys().cloned().map(Value::String).collect(),
        )),
        (Value::Map(values), "values") if no_arguments => {
            Resolution::Value(Value::Sequence(values.values().cloned().collect()))
        }
        (Value::Sequence(values), "size" | "length") if no_arguments => {
            Resolution::Value(Value::Integer(values.len() as i64))
        }
        (Value::Sequence(values), "isEmpty" | "empty") if no_arguments => {
            Resolution::Value(Value::Bool(values.is_empty()))
        }
        (Value::Sequence(values), "first") if no_arguments => values
            .first()
            .cloned()
            .map_or(Resolution::NotFound, Resolution::Value),
        (Value::Sequence(values), "last") if no_arguments => values
            .last()
            .cloned()
            .map_or(Resolution::NotFound, Resolution::Value),
        (Value::Sequence(values), "get") if arguments.len() == 1 => {
            if let Value::Integer(index) = arguments[0] {
                usize::try_from(index)
                    .ok()
                    .and_then(|index| values.get(index))
                    .cloned()
                    .map_or(Resolution::NotFound, Resolution::Value)
            } else {
                Resolution::NotFound
            }
        }
        (Value::String(value), "size" | "length") if no_arguments => {
            Resolution::Value(Value::Integer(value.chars().count() as i64))
        }
        (Value::String(value), "toUpperCase" | "upper") if no_arguments => {
            Resolution::Value(Value::String(value.to_uppercase()))
        }
        (Value::String(value), "toLowerCase" | "lower") if no_arguments => {
            Resolution::Value(Value::String(value.to_lowercase()))
        }
        _ => Resolution::NotFound,
    }
}

fn resolve_index(base: &Value, index: &Value) -> Resolution<Value> {
    match (base, index) {
        (Value::Map(values), Value::String(key)) => values
            .get(key)
            .cloned()
            .map_or(Resolution::NotFound, Resolution::Value),
        (Value::Sequence(values), Value::Integer(index)) => usize::try_from(*index)
            .ok()
            .and_then(|index| values.get(index))
            .cloned()
            .map_or(Resolution::NotFound, Resolution::Value),
        _ => Resolution::NotFound,
    }
}

fn binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, RenderError> {
    match op {
        BinaryOp::Equal => Ok(Value::Bool(values_equal(&left, &right))),
        BinaryOp::NotEqual => Ok(Value::Bool(!values_equal(&left, &right))),
        BinaryOp::Less | BinaryOp::LessEqual | BinaryOp::Greater | BinaryOp::GreaterEqual => {
            let ordering = compare(&left, &right)?;
            Ok(Value::Bool(match op {
                BinaryOp::Less => ordering.is_lt(),
                BinaryOp::LessEqual => ordering.is_le(),
                BinaryOp::Greater => ordering.is_gt(),
                BinaryOp::GreaterEqual => ordering.is_ge(),
                _ => unreachable!(),
            }))
        }
        BinaryOp::Add => match (left, right) {
            (Value::Integer(left), Value::Integer(right)) => left
                .checked_add(right)
                .map(Value::Integer)
                .ok_or_else(|| RenderError::new(ErrorCode::Arithmetic, "integer overflow")),
            (Value::Float(left), Value::Float(right)) => Ok(Value::Float(left + right)),
            (Value::Integer(left), Value::Float(right)) => Ok(Value::Float(left as f64 + right)),
            (Value::Float(left), Value::Integer(right)) => Ok(Value::Float(left + right as f64)),
            (left, right)
                if matches!(left, Value::String(_)) || matches!(right, Value::String(_)) =>
            {
                Ok(Value::String(format!(
                    "{}{}",
                    left.plain_text(),
                    right.plain_text()
                )))
            }
            (left, right) => Err(type_pair("add", &left, &right)),
        },
        BinaryOp::Subtract | BinaryOp::Multiply | BinaryOp::Divide | BinaryOp::Remainder => {
            numeric_binary(op, left, right)
        }
        BinaryOp::And | BinaryOp::Or | BinaryOp::Elvis => unreachable!(),
    }
}

fn numeric_binary(op: BinaryOp, left: Value, right: Value) -> Result<Value, RenderError> {
    if let (Value::Integer(left), Value::Integer(right)) = (&left, &right) {
        let value = match op {
            BinaryOp::Subtract => left.checked_sub(*right),
            BinaryOp::Multiply => left.checked_mul(*right),
            BinaryOp::Divide => left.checked_div(*right),
            BinaryOp::Remainder => left.checked_rem(*right),
            _ => unreachable!(),
        };
        return value.map(Value::Integer).ok_or_else(|| {
            RenderError::new(
                ErrorCode::Arithmetic,
                "invalid integer arithmetic (overflow or division by zero)",
            )
        });
    }
    let left_number = number(&left);
    let right_number = number(&right);
    match (left_number, right_number) {
        (Some(left), Some(right)) => Ok(Value::Float(match op {
            BinaryOp::Subtract => left - right,
            BinaryOp::Multiply => left * right,
            BinaryOp::Divide => left / right,
            BinaryOp::Remainder => left % right,
            _ => unreachable!(),
        })),
        _ => Err(type_pair("perform arithmetic on", &left, &right)),
    }
}

fn values_equal(left: &Value, right: &Value) -> bool {
    match (number(left), number(right)) {
        (Some(left), Some(right)) => left == right,
        _ => left == right,
    }
}

fn compare(left: &Value, right: &Value) -> Result<std::cmp::Ordering, RenderError> {
    match (number(left), number(right)) {
        (Some(left), Some(right)) => left
            .partial_cmp(&right)
            .ok_or_else(|| RenderError::new(ErrorCode::Arithmetic, "values cannot be ordered")),
        _ => match (left, right) {
            (Value::String(left), Value::String(right)) => Ok(left.cmp(right)),
            _ => Err(type_pair("compare", left, right)),
        },
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(value) => Some(*value as f64),
        Value::Float(value) => Some(*value),
        _ => None,
    }
}

fn type_pair(operation: &str, left: &Value, right: &Value) -> RenderError {
    RenderError::new(
        ErrorCode::Type,
        format!(
            "cannot {operation} {} and {}",
            left.type_name(),
            right.type_name()
        ),
    )
}

fn write_value(
    value: &Value,
    media_type: MediaType,
    output: &mut String,
) -> Result<(), RenderError> {
    match value {
        Value::Sequence(_) | Value::Map(_) => Err(RenderError::new(
            ErrorCode::Type,
            format!("cannot render {} directly", value.type_name()),
        )),
        Value::SafeHtml(value) if media_type == MediaType::Html => {
            output.push_str(value);
            Ok(())
        }
        Value::SafeXml(value) if media_type == MediaType::Xml => {
            output.push_str(value);
            Ok(())
        }
        Value::SafeJsonString(value) if media_type == MediaType::Json => {
            output.push_str(value);
            Ok(())
        }
        value => {
            let text = value.plain_text();
            match media_type {
                MediaType::Html | MediaType::Xml => escape::html(&text, output),
                MediaType::Json => escape::json(&text, output),
                MediaType::Text => output.push_str(&text),
            }
            Ok(())
        }
    }
}

fn normalize_id(id: &str) -> Result<String, RenderError> {
    let path = Path::new(id);
    if id.is_empty()
        || path.is_absolute()
        || path.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        return Err(RenderError::new(
            ErrorCode::MissingTemplate,
            format!("invalid template ID `{id}`"),
        ));
    }
    Ok(path
        .components()
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_string_lossy()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/"))
}

fn missing_template(id: &str) -> RenderError {
    RenderError::new(
        ErrorCode::MissingTemplate,
        format!("template `{id}` was not found"),
    )
}

fn base_language(language: &str) -> Option<&str> {
    language
        .find(['-', '_'])
        .map(|separator| &language[..separator])
        .filter(|language| !language.is_empty())
}
