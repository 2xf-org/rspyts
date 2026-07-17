use std::collections::BTreeMap;
use std::fs;

use anyhow::{Context, Result, bail};
use rspyts::ir::{CargoPackageId, DefinitionId, ErrorDef, Manifest, TypeDef};

use crate::config::{DependencyConfig, Project};
use crate::{
    ContractLock, LOCK_VERSION, LockedDependency, LockedHosts, LockedTypeScriptHost, fingerprint,
};

#[derive(Debug)]
pub struct ResolvedContract {
    pub manifest: Manifest,
    pub dependencies: BTreeMap<String, LockedDependency>,
    pub hosts: LockedHosts,
    pub foreign_types: BTreeMap<DefinitionId, TypeDef>,
    pub foreign_errors: BTreeMap<DefinitionId, ErrorDef>,
}

pub fn contract(project: &Project, manifest: Manifest) -> Result<ResolvedContract> {
    let hosts = LockedHosts {
        python: project.python.as_ref().map(|config| config.package.clone()),
        typescript: project
            .typescript
            .as_ref()
            .map(|config| LockedTypeScriptHost {
                package: config.package.clone(),
                mode: config.mode,
            }),
    };
    let root_owner = CargoPackageId::new(manifest.crate_name.clone());
    if manifest.imports.len() > 1 {
        bail!("contracts may import at most one direct dependency");
    }
    if project.dependencies().len() > 1 {
        bail!("rspyts.toml may map at most one direct dependency");
    }

    match (
        manifest.imports.first(),
        project.dependencies().iter().next(),
    ) {
        (Some(import), Some((_, dependency))) if import.owner != dependency.owner => {
            bail!(
                "contract imports Cargo package `{}`, but rspyts.toml maps `{}`",
                import.owner,
                dependency.owner
            );
        }
        (Some(import), None) => bail!(
            "contract imports Cargo package `{}`, but rspyts.toml has no dependency mapping for it",
            import.owner
        ),
        (None, Some((_, dependency))) => bail!(
            "rspyts.toml maps Cargo package `{}`, but the contract does not import it",
            dependency.owner
        ),
        _ => {}
    }
    if project
        .dependencies()
        .values()
        .any(|dependency| dependency.owner == root_owner)
    {
        bail!("rspyts.toml cannot map the root Cargo package as its own dependency");
    }

    let mut dependencies = BTreeMap::new();
    let mut foreign_types = BTreeMap::new();
    let mut foreign_errors = BTreeMap::new();
    if let Some(import) = manifest.imports.first() {
        let (alias, config) = project
            .dependencies()
            .iter()
            .next()
            .expect("a configured dependency was matched above");
        require_host_mappings(project, alias, config)?;
        let lock = read_lock(config)?;
        validate_dependency_lock(&lock, &import.owner, alias)?;
        validate_dependency_hosts(
            &lock,
            config,
            project.typescript.as_ref().map(|host| host.mode),
            alias,
        )?;
        for linked in &import.types {
            let identity = DefinitionId {
                owner: import.owner.clone(),
                id: linked.id.clone(),
            };
            let definition = lock
                .manifest
                .types
                .iter()
                .find(|definition| definition.owner == import.owner && definition.id == linked.id)
                .with_context(|| {
                    format!("dependency `{alias}` lock does not define imported type `{identity}`")
                })?
                .clone();
            if crate::semantic_type_def(linked) != crate::semantic_type_def(&definition) {
                bail!(
                    "dependency `{alias}` linked type `{identity}` differs from its locked definition"
                );
            }
            foreign_types.insert(identity, definition);
        }
        for linked in &import.errors {
            let identity = DefinitionId {
                owner: import.owner.clone(),
                id: linked.id.clone(),
            };
            let definition = lock
                .manifest
                .errors
                .iter()
                .find(|definition| definition.owner == import.owner && definition.id == linked.id)
                .with_context(|| {
                    format!("dependency `{alias}` lock does not define imported error `{identity}`")
                })?
                .clone();
            if crate::semantic_error_def(linked) != crate::semantic_error_def(&definition) {
                bail!(
                    "dependency `{alias}` linked error `{identity}` differs from its locked definition"
                );
            }
            foreign_errors.insert(identity, definition);
        }

        let dependency_typescript = locked_typescript_host(&lock, config);
        dependencies.insert(
            alias.clone(),
            LockedDependency {
                owner: import.owner.clone(),
                crate_version: lock.manifest.crate_version.clone(),
                fingerprint: lock.fingerprint,
                python: config.python.clone(),
                typescript: dependency_typescript,
                types: import
                    .types
                    .iter()
                    .map(|linked| {
                        foreign_types
                            .get(&DefinitionId {
                                owner: import.owner.clone(),
                                id: linked.id.clone(),
                            })
                            .map(crate::semantic_type_def)
                            .expect("resolved imported type")
                    })
                    .collect(),
                errors: import
                    .errors
                    .iter()
                    .map(|linked| {
                        foreign_errors
                            .get(&DefinitionId {
                                owner: import.owner.clone(),
                                id: linked.id.clone(),
                            })
                            .map(crate::semantic_error_def)
                            .expect("resolved imported error")
                    })
                    .collect(),
            },
        );
    }
    validate_dependency_mapping(&hosts, &dependencies)?;

    let resolved = ResolvedContract {
        manifest,
        hosts,
        dependencies,
        foreign_types,
        foreign_errors,
    };
    if resolved.hosts.python.is_some() {
        crate::validate::python_manifest(&resolved.manifest)?;
    }
    if let Some(config) = project.typescript.as_ref() {
        crate::validate::typescript_contract(&resolved, config.mode)?;
        if config.mode == crate::config::TypeScriptMode::Static {
            crate::emit::validate_static_typescript(&resolved)?;
        }
    }
    Ok(resolved)
}

fn require_host_mappings(
    project: &Project,
    alias: &str,
    dependency: &DependencyConfig,
) -> Result<()> {
    if project.python.is_some() && dependency.python.is_none() {
        bail!("dependency `{alias}` requires `python` because [python] is configured");
    }
    if project.typescript.is_some() && dependency.typescript.is_none() {
        bail!("dependency `{alias}` requires `typescript` because [typescript] is configured");
    }
    Ok(())
}

fn validate_dependency_hosts(
    lock: &ContractLock,
    configured: &DependencyConfig,
    root_typescript_mode: Option<crate::config::TypeScriptMode>,
    alias: &str,
) -> Result<()> {
    if root_typescript_mode != Some(crate::config::TypeScriptMode::Static) {
        bail!("dependency `{alias}` requires the root TypeScript host to use static mode");
    }
    if let Some(package) = configured.python.as_deref()
        && lock.hosts.python.as_deref() != Some(package)
    {
        bail!(
            "dependency `{alias}` Python package is configured as `{package}`, but its lock exports {:?}",
            lock.hosts.python
        );
    }
    if let Some(package) = configured.typescript.as_deref()
        && lock
            .hosts
            .typescript
            .as_ref()
            .map(|host| host.package.as_str())
            != Some(package)
    {
        bail!(
            "dependency `{alias}` TypeScript package is configured as `{package}`, but its lock exports {:?}",
            lock.hosts
                .typescript
                .as_ref()
                .map(|host| host.package.as_str())
        );
    }
    let dependency_host = lock
        .hosts
        .typescript
        .as_ref()
        .with_context(|| format!("dependency `{alias}` must export a TypeScript WASM host"))?;
    if dependency_host.mode != crate::config::TypeScriptMode::Wasm {
        bail!(
            "dependency `{alias}` TypeScript host must use wasm mode, but its lock uses static mode"
        );
    }
    Ok(())
}

fn locked_typescript_host(
    lock: &ContractLock,
    configured: &DependencyConfig,
) -> Option<LockedTypeScriptHost> {
    configured
        .typescript
        .as_ref()
        .and(lock.hosts.typescript.clone())
}

fn validate_dependency_mapping(
    root_hosts: &LockedHosts,
    dependencies: &BTreeMap<String, LockedDependency>,
) -> Result<()> {
    if dependencies.len() > 1 {
        bail!("resolved contracts may contain at most one direct dependency");
    }
    let Some((alias, dependency)) = dependencies.iter().next() else {
        return Ok(());
    };
    if let Some(package) = dependency.python.as_ref()
        && root_hosts.python.as_ref() == Some(package)
    {
        bail!(
            "Python package `{package}` is mapped by dependency `{alias}` and exported by the root contract"
        );
    }
    if let Some(host) = dependency.typescript.as_ref()
        && root_hosts
            .typescript
            .as_ref()
            .is_some_and(|root| root.package == host.package)
    {
        bail!(
            "TypeScript package `{}` is mapped by dependency `{alias}` and exported by the root contract",
            host.package
        );
    }
    Ok(())
}

fn read_lock(config: &DependencyConfig) -> Result<ContractLock> {
    let metadata = fs::symlink_metadata(&config.lock).with_context(|| {
        format!(
            "failed to inspect dependency lock {}",
            config.lock.display()
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "dependency lock must be a regular non-symlink file: {}",
            config.lock.display()
        );
    }
    let source = fs::read_to_string(&config.lock)
        .with_context(|| format!("failed to read dependency lock {}", config.lock.display()))?;
    serde_json::from_str(&source)
        .with_context(|| format!("invalid dependency lock {}", config.lock.display()))
}

fn validate_dependency_lock(
    lock: &ContractLock,
    expected_owner: &CargoPackageId,
    alias: &str,
) -> Result<()> {
    if lock.schema_version != LOCK_VERSION {
        bail!(
            "dependency `{alias}` uses lock schema {}; expected {LOCK_VERSION}",
            lock.schema_version
        );
    }
    if lock.manifest.crate_name != expected_owner.as_str() {
        bail!(
            "dependency `{alias}` lock belongs to `{}`, expected `{expected_owner}`",
            lock.manifest.crate_name
        );
    }
    crate::validate::manifest(&lock.manifest)
        .with_context(|| format!("dependency `{alias}` has an invalid locked manifest"))?;
    let actual = fingerprint(&lock.manifest, &lock.hosts, &lock.dependencies)?;
    if actual != lock.fingerprint {
        bail!(
            "dependency `{alias}` lock fingerprint mismatch: recorded {}, computed {actual}",
            lock.fingerprint
        );
    }
    if !lock.manifest.imports.is_empty() || !lock.dependencies.is_empty() {
        bail!("dependency `{alias}` must be a leaf contract");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LOCK_VERSION, LockedHosts, LockedTypeScriptHost};
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn manifest() -> Manifest {
        Manifest {
            ir_version: rspyts::ir::IR_VERSION,
            crate_name: "dependency".into(),
            crate_version: "1.0.0".into(),
            module_name: "dependency".into(),
            imports: vec![],
            types: vec![],
            errors: vec![],
            functions: vec![],
            resources: vec![],
            constants: vec![],
        }
    }

    fn lock() -> ContractLock {
        let manifest = manifest();
        let hosts = LockedHosts {
            python: None,
            typescript: None,
        };
        let dependencies = BTreeMap::new();
        ContractLock {
            schema_version: LOCK_VERSION,
            fingerprint: fingerprint(&manifest, &hosts, &dependencies).unwrap(),
            hosts,
            dependencies,
            manifest,
        }
    }

    #[test]
    fn rejects_stale_and_tampered_dependency_locks() {
        let owner = CargoPackageId::new("dependency");

        let mut stale = lock();
        stale.schema_version -= 1;
        assert!(
            validate_dependency_lock(&stale, &owner, "dependency")
                .unwrap_err()
                .to_string()
                .contains("lock schema")
        );

        let mut tampered = lock();
        tampered.manifest.module_name = "changed".into();
        assert!(
            validate_dependency_lock(&tampered, &owner, "dependency")
                .unwrap_err()
                .to_string()
                .contains("fingerprint mismatch")
        );

        let error = validate_dependency_lock(
            &lock(),
            &CargoPackageId::new("different-owner"),
            "dependency",
        )
        .unwrap_err();
        assert!(error.to_string().contains("expected `different-owner`"));
    }

    #[test]
    fn rejects_more_than_one_imported_owner() {
        let root = std::env::temp_dir().join(format!(
            "rspyts-resolve-cardinality-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(root.join("rust/src")).unwrap();
        fs::write(
            root.join("rust/Cargo.toml"),
            "[package]\nname = \"root\"\nversion = \"1.0.0\"\nedition = \"2024\"\n",
        )
        .unwrap();
        fs::write(root.join("rust/src/lib.rs"), "").unwrap();
        fs::write(
            root.join("rspyts.toml"),
            "[crate]\npath = \"rust\"\n\n[typescript]\npackage = \"@example/root\"\nmode = \"static\"\n",
        )
        .unwrap();
        let project = Project::read(&root.join("rspyts.toml")).unwrap();
        let mut manifest = manifest();
        manifest.crate_name = "root".into();
        manifest.imports = ["first", "second"]
            .into_iter()
            .map(|owner| rspyts::ir::ImportedPackage {
                owner: CargoPackageId::new(owner),
                types: vec![],
                errors: vec![],
            })
            .collect();

        let error = contract(&project, manifest).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("may import at most one direct dependency")
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn dependency_locks_must_be_leaves() {
        let owner = CargoPackageId::new("dependency");
        let mut imported = lock();
        imported.manifest.imports.push(rspyts::ir::ImportedPackage {
            owner: CargoPackageId::new("transitive"),
            types: vec![],
            errors: vec![],
        });
        imported.fingerprint =
            fingerprint(&imported.manifest, &imported.hosts, &imported.dependencies).unwrap();
        assert!(
            validate_dependency_lock(&imported, &owner, "dependency")
                .unwrap_err()
                .to_string()
                .contains("must be a leaf contract")
        );

        let mut recorded = lock();
        recorded.dependencies.insert(
            "transitive".into(),
            locked_dependency(
                "transitive",
                Some("example.transitive"),
                Some("@example/transitive"),
            ),
        );
        recorded.fingerprint =
            fingerprint(&recorded.manifest, &recorded.hosts, &recorded.dependencies).unwrap();
        assert!(
            validate_dependency_lock(&recorded, &owner, "dependency")
                .unwrap_err()
                .to_string()
                .contains("must be a leaf contract")
        );
    }

    #[test]
    fn only_static_roots_may_depend_on_wasm_owners() {
        let mut dependency = lock();
        dependency.hosts.typescript = Some(LockedTypeScriptHost {
            package: "@example/dependency".into(),
            mode: crate::config::TypeScriptMode::Wasm,
        });
        dependency.fingerprint = fingerprint(
            &dependency.manifest,
            &dependency.hosts,
            &dependency.dependencies,
        )
        .unwrap();
        let configured = DependencyConfig {
            owner: CargoPackageId::new("dependency"),
            lock: PathBuf::from("dependency/rspyts.lock"),
            python: None,
            typescript: Some("@example/dependency".into()),
        };

        assert!(
            validate_dependency_hosts(
                &dependency,
                &configured,
                Some(crate::config::TypeScriptMode::Wasm),
                "dependency",
            )
            .unwrap_err()
            .to_string()
            .contains("root TypeScript host to use static mode")
        );
        validate_dependency_hosts(
            &dependency,
            &configured,
            Some(crate::config::TypeScriptMode::Static),
            "dependency",
        )
        .unwrap();

        dependency.hosts.typescript.as_mut().unwrap().mode = crate::config::TypeScriptMode::Static;
        let error = validate_dependency_hosts(
            &dependency,
            &configured,
            Some(crate::config::TypeScriptMode::Static),
            "dependency",
        )
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("TypeScript host must use wasm mode")
        );
    }

    #[test]
    fn dependency_host_package_mappings_are_exact() {
        let mut dependency = lock();
        dependency.hosts = LockedHosts {
            python: Some("example.dependency".into()),
            typescript: Some(LockedTypeScriptHost {
                package: "@example/dependency".into(),
                mode: crate::config::TypeScriptMode::Wasm,
            }),
        };
        let mut configured = DependencyConfig {
            owner: CargoPackageId::new("dependency"),
            lock: PathBuf::from("dependency/rspyts.lock"),
            python: Some("example.dependency".into()),
            typescript: Some("@example/dependency".into()),
        };
        validate_dependency_hosts(
            &dependency,
            &configured,
            Some(crate::config::TypeScriptMode::Static),
            "dependency",
        )
        .unwrap();

        configured.python = Some("example.different".into());
        assert!(
            validate_dependency_hosts(
                &dependency,
                &configured,
                Some(crate::config::TypeScriptMode::Static),
                "dependency",
            )
            .unwrap_err()
            .to_string()
            .contains("Python package is configured as `example.different`")
        );

        configured.python = Some("example.dependency".into());
        configured.typescript = Some("@example/different".into());
        assert!(
            validate_dependency_hosts(
                &dependency,
                &configured,
                Some(crate::config::TypeScriptMode::Static),
                "dependency",
            )
            .unwrap_err()
            .to_string()
            .contains("TypeScript package is configured as `@example/different`")
        );
    }

    #[test]
    fn locked_dependency_retains_the_validated_typescript_host() {
        let mut dependency = lock();
        dependency.hosts.typescript = Some(LockedTypeScriptHost {
            package: "@example/dependency".into(),
            mode: crate::config::TypeScriptMode::Wasm,
        });
        let configured = DependencyConfig {
            owner: CargoPackageId::new("dependency"),
            lock: PathBuf::from("dependency/rspyts.lock"),
            python: None,
            typescript: Some("@example/dependency".into()),
        };

        assert_eq!(
            locked_typescript_host(&dependency, &configured),
            dependency.hosts.typescript
        );
        assert_eq!(
            serde_json::to_value(locked_typescript_host(&dependency, &configured)).unwrap(),
            serde_json::json!({
                "package": "@example/dependency",
                "mode": "wasm",
            })
        );

        let without_typescript_mapping = DependencyConfig {
            typescript: None,
            ..configured
        };
        assert_eq!(
            locked_typescript_host(&dependency, &without_typescript_mapping),
            None
        );
    }

    #[test]
    fn locked_dependency_retains_the_validated_contract_version() {
        let dependency = lock();
        let locked = LockedDependency {
            owner: CargoPackageId::new(dependency.manifest.crate_name.clone()),
            crate_version: dependency.manifest.crate_version.clone(),
            fingerprint: dependency.fingerprint,
            python: None,
            typescript: None,
            types: vec![],
            errors: vec![],
        };

        assert_eq!(locked.crate_version, "1.0.0");
        assert_eq!(
            serde_json::to_value(locked).unwrap()["crateVersion"],
            "1.0.0"
        );
    }

    fn locked_dependency(
        owner: &str,
        python: Option<&str>,
        typescript: Option<&str>,
    ) -> LockedDependency {
        LockedDependency {
            owner: CargoPackageId::new(owner),
            crate_version: "1.0.0".into(),
            fingerprint: "sha256:dependency".into(),
            python: python.map(str::to_owned),
            typescript: typescript.map(|package| LockedTypeScriptHost {
                package: package.into(),
                mode: crate::config::TypeScriptMode::Wasm,
            }),
            types: vec![],
            errors: vec![],
        }
    }

    #[test]
    fn rejects_dependency_host_packages_that_shadow_the_root_package() {
        let dependencies = BTreeMap::from([(
            "dependency".into(),
            locked_dependency("dependency", Some("example.root"), Some("@example/root")),
        )]);
        let hosts = LockedHosts {
            python: Some("example.root".into()),
            typescript: Some(LockedTypeScriptHost {
                package: "@example/root".into(),
                mode: crate::config::TypeScriptMode::Static,
            }),
        };

        let error = validate_dependency_mapping(&hosts, &dependencies).unwrap_err();
        assert!(error.to_string().contains("Python package `example.root`"));

        let mut typescript_only = dependencies;
        typescript_only
            .get_mut("dependency")
            .expect("dependency fixture")
            .python = None;
        let hosts = LockedHosts {
            python: None,
            ..hosts
        };
        let error = validate_dependency_mapping(&hosts, &typescript_only).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("TypeScript package `@example/root`")
        );
    }
}
