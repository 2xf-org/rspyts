use std::collections::{BTreeMap, BTreeSet};
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
    let imported_owners = manifest
        .imports
        .iter()
        .map(|import| import.owner.clone())
        .collect::<BTreeSet<_>>();
    let configured_owners = project
        .dependencies()
        .values()
        .map(|dependency| dependency.owner.clone())
        .collect::<BTreeSet<_>>();

    if let Some(owner) = imported_owners.difference(&configured_owners).next() {
        bail!(
            "contract imports Cargo package `{owner}`, but rspyts.toml has no dependency mapping for it"
        );
    }
    if let Some(owner) = configured_owners.difference(&imported_owners).next() {
        bail!("rspyts.toml maps Cargo package `{owner}`, but the contract does not import it");
    }
    if configured_owners.contains(&root_owner) {
        bail!("rspyts.toml cannot map the root Cargo package as its own dependency");
    }

    let mut dependencies = BTreeMap::new();
    let mut foreign_types = BTreeMap::new();
    let mut foreign_errors = BTreeMap::new();
    let mut dependency_graph = BTreeMap::from([(root_owner.clone(), imported_owners.clone())]);
    for import in &manifest.imports {
        let (alias, config) = dependency_for_owner(project, &import.owner)?;
        require_host_mappings(project, alias, config)?;
        let lock = read_lock(config)?;
        validate_dependency_lock(&lock, &import.owner, &root_owner, alias)?;
        validate_dependency_hosts(&lock, config, alias)?;
        let transitive = lock
            .manifest
            .imports
            .iter()
            .map(|import| import.owner.clone())
            .collect::<BTreeSet<_>>();
        if let Some(owner) = transitive.difference(&imported_owners).next() {
            bail!(
                "dependency `{alias}` uses transitive Cargo package `{owner}` without a direct root mapping"
            );
        }
        dependency_graph.insert(import.owner.clone(), transitive);

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

        dependencies.insert(
            alias.to_owned(),
            LockedDependency {
                owner: import.owner.clone(),
                fingerprint: lock.fingerprint,
                python: config.python.clone(),
                typescript: config.typescript.clone(),
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
    reject_dependency_cycles(&dependency_graph)?;

    Ok(ResolvedContract {
        manifest,
        hosts,
        dependencies,
        foreign_types,
        foreign_errors,
    })
}

fn dependency_for_owner<'a>(
    project: &'a Project,
    owner: &CargoPackageId,
) -> Result<(&'a str, &'a DependencyConfig)> {
    project
        .dependencies()
        .iter()
        .find(|(_, dependency)| dependency.owner == *owner)
        .map(|(alias, dependency)| (alias.as_str(), dependency))
        .with_context(|| format!("missing dependency mapping for Cargo package `{owner}`"))
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
    alias: &str,
) -> Result<()> {
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
    root_owner: &CargoPackageId,
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
    if lock
        .manifest
        .imports
        .iter()
        .any(|import| import.owner == *root_owner)
        || lock
            .dependencies
            .values()
            .any(|dependency| dependency.owner == *root_owner)
    {
        bail!("dependency `{alias}` creates a contract dependency cycle");
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
    validate_lock_dependency_records(lock, alias)?;
    Ok(())
}

fn validate_lock_dependency_records(lock: &ContractLock, alias: &str) -> Result<()> {
    let imported = lock
        .manifest
        .imports
        .iter()
        .map(|import| (&import.owner, import))
        .collect::<BTreeMap<_, _>>();
    let records = lock
        .dependencies
        .values()
        .map(|dependency| (&dependency.owner, dependency))
        .collect::<BTreeMap<_, _>>();
    if imported.keys().collect::<Vec<_>>() != records.keys().collect::<Vec<_>>() {
        bail!("dependency `{alias}` lock has inconsistent imported package records");
    }
    for (owner, import) in imported {
        let record = records
            .get(owner)
            .expect("matching dependency record was checked");
        let types = import
            .types
            .iter()
            .map(crate::semantic_type_def)
            .collect::<Vec<_>>();
        let errors = import
            .errors
            .iter()
            .map(crate::semantic_error_def)
            .collect::<Vec<_>>();
        if record.types != types || record.errors != errors {
            bail!(
                "dependency `{alias}` lock snapshots for Cargo package `{owner}` are inconsistent"
            );
        }
    }
    Ok(())
}

fn reject_dependency_cycles(
    graph: &BTreeMap<CargoPackageId, BTreeSet<CargoPackageId>>,
) -> Result<()> {
    fn visit(
        owner: &CargoPackageId,
        graph: &BTreeMap<CargoPackageId, BTreeSet<CargoPackageId>>,
        visited: &mut BTreeSet<CargoPackageId>,
        active: &mut Vec<CargoPackageId>,
    ) -> Result<()> {
        if let Some(position) = active.iter().position(|current| current == owner) {
            let mut cycle = active[position..].to_vec();
            cycle.push(owner.clone());
            bail!(
                "contract dependency cycle: {}",
                cycle
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" -> ")
            );
        }
        if !visited.insert(owner.clone()) {
            return Ok(());
        }
        active.push(owner.clone());
        if let Some(dependencies) = graph.get(owner) {
            for dependency in dependencies {
                visit(dependency, graph, visited, active)?;
            }
        }
        active.pop();
        Ok(())
    }

    let mut visited = BTreeSet::new();
    let mut active = Vec::new();
    for owner in graph.keys() {
        visit(owner, graph, &mut visited, &mut active)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LOCK_VERSION, LockedHosts};

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
        let root = CargoPackageId::new("root");

        let mut stale = lock();
        stale.schema_version -= 1;
        assert!(
            validate_dependency_lock(&stale, &owner, &root, "dependency")
                .unwrap_err()
                .to_string()
                .contains("lock schema")
        );

        let mut tampered = lock();
        tampered.manifest.module_name = "changed".into();
        assert!(
            validate_dependency_lock(&tampered, &owner, &root, "dependency")
                .unwrap_err()
                .to_string()
                .contains("fingerprint mismatch")
        );
    }
}
