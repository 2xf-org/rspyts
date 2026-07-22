use super::*;

pub(crate) fn validate_contract(project: &Project, manifest: &Manifest) -> Result<()> {
    if manifest.package_name != project.package_name
        || manifest.package_version != project.package_version
    {
        bail!("the discovered package does not match Cargo metadata");
    }
    if !is_python_identifier(&manifest.module_name)
        || matches!(manifest.module_name.as_str(), "api" | "models" | "runtime")
        || is_python_package_attribute(&manifest.module_name)
    {
        bail!(
            "Python native module name `{}` is invalid or reserved for generated package loading",
            manifest.module_name
        );
    }
    validate_namespaces(manifest)?;
    validate_model_namespace_cycles(manifest)?;
    Ok(())
}

pub(crate) fn validate_namespaces(manifest: &Manifest) -> Result<()> {
    let namespace_map = namespaces(manifest);
    let python_namespace_paths = namespace_map
        .keys()
        .map(Namespace::python_segments)
        .collect::<Vec<_>>();
    for (owner, rust_module) in export_origins(manifest) {
        let namespace = manifest.namespace(owner, rust_module);
        if let Some(package) = &namespace.package {
            let segment = package.replace('-', "_");
            if !is_python_identifier(&segment) || is_python_package_attribute(&segment) {
                bail!(
                    "Cargo package `{owner}` derives the invalid Python namespace segment `{segment}`; rename the Cargo package"
                );
            }
        }
        for segment in rust_module.split("::").skip(1) {
            if !is_python_identifier(segment) || is_python_package_attribute(segment) {
                bail!(
                    "Rust module `{rust_module}` in Cargo package `{owner}` contains the invalid Python namespace segment `{segment}`; rename the Rust module"
                );
            }
        }
    }
    let mut python_paths = BTreeMap::<Vec<String>, Namespace>::new();
    for namespace in namespace_map.keys() {
        let path = namespace.python_segments();
        if let Some(existing) = python_paths.insert(path.clone(), namespace.clone())
            && existing != *namespace
        {
            bail!(
                "Rust namespaces `{}` and `{}` both derive the Python path `{}`; rename one Cargo package",
                display_namespace(&existing),
                display_namespace(namespace),
                path.join(".")
            );
        }
    }
    for (namespace, items) in namespace_map {
        let namespace_path = namespace.python_segments();
        let child_package_names = python_namespace_paths
            .iter()
            .filter(|path| {
                path.len() == namespace_path.len() + 1
                    && path.starts_with(namespace_path.as_slice())
            })
            .filter_map(|path| path.last().map(String::as_str))
            .collect::<BTreeSet<_>>();
        let mut python_names = items
            .types
            .iter()
            .map(|item| item.name.clone())
            .chain(items.errors.iter().map(|item| item.name.clone()))
            .chain(items.resources.iter().map(|item| item.name.clone()))
            .chain(items.functions.iter().map(|item| item.rust_name.clone()))
            .chain(items.constants.iter().map(|item| item.host_name.clone()))
            .collect::<Vec<_>>();
        for item in &items.types {
            if let rspyts::ir::TypeShape::TaggedEnum { variants, .. } = &item.shape {
                python_names.extend(
                    variants
                        .iter()
                        .map(|variant| tagged_variant_name(&item.name, &variant.rust_name)),
                );
            }
        }
        let mut buffers = BTreeSet::new();
        for reference in namespace_refs(&items) {
            crate::contract::collect_buffers(reference, &mut buffers);
        }
        let has_models = !items.types.is_empty() || !buffers.is_empty();
        let has_api = !items.errors.is_empty()
            || !items.functions.is_empty()
            || !items.resources.is_empty()
            || !items.constants.is_empty();
        if let Some(name) = child_package_names.iter().find(|name| {
            (**name == "models" && has_models)
                || (**name == "api" && has_api)
                || (namespace == Namespace::root()
                    && (**name == "runtime" || **name == manifest.module_name.as_str()))
        }) {
            bail!(
                "Python namespace segment `{name}` conflicts with a generated module in namespace `{}`",
                display_namespace(&namespace)
            );
        }
        python_names.extend(
            buffers
                .into_iter()
                .map(|element| python::buffer_name(element).to_owned()),
        );
        if let Some(name) = python_names.iter().find(|name| {
            matches!(
                name.as_str(),
                "__all__" | "__dir__" | "__getattr__" | "api" | "models"
            ) || name.starts_with("_rspyts_models_")
                || is_python_package_attribute(name)
                || (namespace == Namespace::root()
                    && (name.as_str() == "runtime"
                        || name.as_str() == manifest.module_name.as_str()))
                || child_package_names.contains(name.as_str())
        }) {
            bail!(
                "Python export name `{name}` is reserved for generated package loading in namespace `{}`",
                display_namespace(&namespace)
            );
        }
        unique_public_names("Python", python_names.into_iter())
            .with_context(|| format!("in namespace `{}`", display_namespace(&namespace)))?;

        let mut typescript_names = items
            .types
            .iter()
            .map(|item| item.name.clone())
            .chain(items.errors.iter().map(|item| item.name.clone()))
            .chain(items.resources.iter().map(|item| item.name.clone()))
            .chain(items.functions.iter().map(|item| item.host_name.clone()))
            .chain(items.constants.iter().map(|item| item.host_name.clone()))
            .collect::<Vec<_>>();
        for item in &items.types {
            if let rspyts::ir::TypeShape::TaggedEnum { variants, .. } = &item.shape {
                typescript_names.extend(
                    variants
                        .iter()
                        .map(|variant| tagged_variant_name(&item.name, &variant.rust_name)),
                );
            }
        }
        unique_public_names("TypeScript", typescript_names.into_iter())
            .with_context(|| format!("in namespace `{}`", display_namespace(&namespace)))?;
    }
    Ok(())
}

pub(crate) fn export_origins(manifest: &Manifest) -> Vec<(&rspyts::ir::CargoPackageId, &str)> {
    let mut origins = manifest
        .types
        .iter()
        .map(|item| (&item.owner, item.rust_module.as_str()))
        .chain(
            manifest
                .errors
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .chain(
            manifest
                .functions
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .chain(
            manifest
                .resources
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .chain(
            manifest
                .constants
                .iter()
                .map(|item| (&item.owner, item.rust_module.as_str())),
        )
        .collect::<Vec<_>>();
    origins.sort();
    origins.dedup();
    origins
}

pub(crate) fn validate_model_namespace_cycles(manifest: &Manifest) -> Result<()> {
    let mut graph = BTreeMap::<Namespace, BTreeSet<Namespace>>::new();
    for definition in &manifest.types {
        let source = manifest.namespace(&definition.owner, &definition.rust_module);
        graph.entry(source.clone()).or_default();
        for reference in type_refs(definition) {
            let mut identities = Vec::new();
            named_identities(reference, &mut identities);
            for identity in identities {
                let target = type_namespace(identity, manifest)?;
                if target != source {
                    graph.entry(source.clone()).or_default().insert(target);
                }
            }
        }
    }
    let mut complete = BTreeSet::new();
    let mut active = BTreeSet::new();
    let mut stack = Vec::new();
    for namespace in graph.keys() {
        if let Some(cycle) =
            namespace_cycle(namespace, &graph, &mut active, &mut complete, &mut stack)
        {
            let path = cycle
                .iter()
                .map(display_namespace)
                .collect::<Vec<_>>()
                .join(" -> ");
            bail!(
                "Python model namespaces form a dependency cycle: {path}; move the declarations into one Rust module or remove the cyclic type reference"
            );
        }
    }
    Ok(())
}

pub(crate) fn namespace_cycle(
    namespace: &Namespace,
    graph: &BTreeMap<Namespace, BTreeSet<Namespace>>,
    active: &mut BTreeSet<Namespace>,
    complete: &mut BTreeSet<Namespace>,
    stack: &mut Vec<Namespace>,
) -> Option<Vec<Namespace>> {
    if complete.contains(namespace) {
        return None;
    }
    if active.contains(namespace) {
        let start = stack.iter().position(|item| item == namespace).unwrap_or(0);
        let mut cycle = stack[start..].to_vec();
        cycle.push(namespace.clone());
        return Some(cycle);
    }
    active.insert(namespace.clone());
    stack.push(namespace.clone());
    if let Some(targets) = graph.get(namespace) {
        for target in targets {
            if let Some(cycle) = namespace_cycle(target, graph, active, complete, stack) {
                return Some(cycle);
            }
        }
    }
    stack.pop();
    active.remove(namespace);
    complete.insert(namespace.clone());
    None
}

pub(crate) fn display_namespace(namespace: &Namespace) -> String {
    let namespace = namespace.display();
    if namespace.is_empty() {
        "<root>".to_owned()
    } else {
        namespace
    }
}

pub(crate) fn unique_public_names<S: AsRef<str>>(
    host: &str,
    names: impl Iterator<Item = S>,
) -> Result<()> {
    let mut seen = BTreeSet::new();
    for name in names {
        let name = name.as_ref();
        if !seen.insert(name.to_owned()) {
            bail!("duplicate {host} export name `{name}`");
        }
    }
    Ok(())
}

pub(crate) fn validate_python_package(value: &str) -> Result<()> {
    if value.is_empty()
        || value
            .split('.')
            .any(|part| !is_python_identifier(part) || is_python_package_attribute(part))
    {
        bail!("Python package `{value}` must contain dot-separated identifiers");
    }
    Ok(())
}

pub(crate) fn validate_typescript_package(value: &str) -> Result<()> {
    let name = value.strip_prefix('@').unwrap_or(value);
    let parts = name.split('/').collect::<Vec<_>>();
    let expected = if value.starts_with('@') { 2 } else { 1 };
    if value.is_empty()
        || parts.len() != expected
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.chars().all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '-' | '_' | '.')
                })
        })
    {
        bail!("invalid TypeScript package `{value}`");
    }
    Ok(())
}

pub(crate) fn is_identifier(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_alphabetic() || character == '_')
        && chars.all(|character| character.is_ascii_alphanumeric() || character == '_')
}

pub(crate) fn is_python_identifier(value: &str) -> bool {
    is_identifier(value)
        && !matches!(
            value,
            "False"
                | "None"
                | "True"
                | "and"
                | "as"
                | "assert"
                | "async"
                | "await"
                | "break"
                | "class"
                | "continue"
                | "def"
                | "del"
                | "elif"
                | "else"
                | "except"
                | "finally"
                | "for"
                | "from"
                | "global"
                | "if"
                | "import"
                | "in"
                | "is"
                | "lambda"
                | "nonlocal"
                | "not"
                | "or"
                | "pass"
                | "raise"
                | "return"
                | "try"
                | "while"
                | "with"
                | "yield"
        )
}

pub(crate) fn is_python_package_attribute(value: &str) -> bool {
    value != "__version__" && value.starts_with("__") && value.ends_with("__")
}
