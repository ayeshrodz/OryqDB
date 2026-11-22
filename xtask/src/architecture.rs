//! Static and diff-aware architecture boundary enforcement (Section 11 of modular-architecture.md).

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::path::{Component, Path};
use std::process::Command;

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct ModulesManifest {
    pub schema_version: u32,
    pub tooling: ToolingSection,
    pub modules: Vec<ModuleRecord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolingSection {
    pub owned: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ModuleRecord {
    pub id: String,
    #[serde(rename = "crate")]
    pub crate_name: String,
    pub state: ModuleState,
    pub owned: Vec<String>,
    pub production_dependencies: Vec<String>,
    pub contract_files: Vec<String>,
    pub optional: bool,
    pub test_commands: Vec<Vec<String>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModuleState {
    Active,
    Planned,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilitiesManifest {
    pub schema_version: u32,
    pub capabilities: Vec<CapabilityRecord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CapabilityRecord {
    pub id: String,
    pub owner: String,
    pub interface_version: u16,
    pub persistent: bool,
    pub read_bits: Vec<u8>,
    pub write_bits: Vec<u8>,
    pub retired: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContractChangesManifest {
    pub schema_version: u32,
    #[serde(default)]
    pub changes: Vec<ContractChangeRecord>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContractChangeRecord {
    pub id: String,
    pub modules: Vec<String>,
    pub files: Vec<String>,
    pub reason: String,
    pub compatibility: String,
    pub evidence: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ChangedPathsInput {
    pub changed: Vec<String>,
    #[serde(default)]
    pub added_note_ids: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct CargoPackageInfo {
    pub normal_deps: BTreeSet<String>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadata {
    packages: Vec<CargoMetadataPackage>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataPackage {
    name: String,
    dependencies: Vec<CargoMetadataDep>,
}

#[derive(Debug, Deserialize)]
struct CargoMetadataDep {
    name: String,
    kind: Option<String>,
}

pub fn load_manifests(
    root: &Path,
) -> Result<
    (
        ModulesManifest,
        CapabilitiesManifest,
        ContractChangesManifest,
    ),
    String,
> {
    let modules_raw = fs::read_to_string(root.join("architecture/modules.toml"))
        .map_err(|e| format!("failed to read architecture/modules.toml: {e}"))?;
    let modules: ModulesManifest = toml::from_str(&modules_raw)
        .map_err(|e| format!("failed to parse architecture/modules.toml: {e}"))?;

    let caps_raw = fs::read_to_string(root.join("architecture/capabilities.toml"))
        .map_err(|e| format!("failed to read architecture/capabilities.toml: {e}"))?;
    let capabilities: CapabilitiesManifest = toml::from_str(&caps_raw)
        .map_err(|e| format!("failed to parse architecture/capabilities.toml: {e}"))?;

    let changes_raw = fs::read_to_string(root.join("architecture/contract-changes.toml"))
        .map_err(|e| format!("failed to read architecture/contract-changes.toml: {e}"))?;
    let changes: ContractChangesManifest = toml::from_str(&changes_raw)
        .map_err(|e| format!("failed to parse architecture/contract-changes.toml: {e}"))?;

    Ok((modules, capabilities, changes))
}

pub fn load_cargo_packages(root: &Path) -> Result<BTreeMap<String, CargoPackageInfo>, String> {
    let output = Command::new("cargo")
        .args(["metadata", "--format-version", "1", "--no-deps"])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to execute `cargo metadata`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("`cargo metadata` failed: {stderr}"));
    }

    let meta: CargoMetadata = serde_json::from_slice(&output.stdout)
        .map_err(|e| format!("failed to parse `cargo metadata` JSON: {e}"))?;

    let mut result = BTreeMap::new();
    for pkg in meta.packages {
        let mut normal_deps = BTreeSet::new();
        for dep in pkg.dependencies {
            if dep.kind.is_none() {
                normal_deps.insert(dep.name);
            }
        }
        result.insert(pkg.name, CargoPackageInfo { normal_deps });
    }
    Ok(result)
}

pub fn run_cli(
    root: &Path,
    base_ref: Option<&str>,
    changed_paths_file: Option<&Path>,
) -> Result<(), String> {
    let (modules, capabilities, changes) = load_manifests(root)?;
    let cargo_pkgs = load_cargo_packages(root)?;
    validate_static(root, &modules, &capabilities, &changes, &cargo_pkgs)?;

    if let Some(base) = base_ref {
        let changed = git_changed_paths(root, base)?;
        let added_ids = git_added_change_note_ids(root, base, &changes)?;
        validate_change_notes(&modules, &changes, &changed, &added_ids)?;
        println!(
            "xtask architecture: static checks and git diff change-note checks against `{base}` passed ({} changed path(s)).",
            changed.len()
        );
    } else if let Some(json_path) = changed_paths_file {
        let raw = fs::read_to_string(json_path)
            .map_err(|e| format!("failed to read `{}`: {e}", json_path.display()))?;
        let input: ChangedPathsInput = serde_json::from_str(&raw)
            .map_err(|e| format!("failed to parse `{}`: {e}", json_path.display()))?;
        let added_ids: HashSet<String> = input.added_note_ids.into_iter().collect();
        validate_change_notes(&modules, &changes, &input.changed, &added_ids)?;
        println!(
            "xtask architecture: static checks and --changed-paths checks passed ({} changed path(s)).",
            input.changed.len()
        );
    } else {
        println!(
            "xtask architecture: static checks passed (change-note diff comparison was not performed)."
        );
    }

    Ok(())
}

pub fn validate_static(
    root: &Path,
    modules: &ModulesManifest,
    capabilities: &CapabilitiesManifest,
    changes: &ContractChangesManifest,
    cargo_pkgs: &BTreeMap<String, CargoPackageInfo>,
) -> Result<(), String> {
    let mut errors = Vec::new();

    if modules.schema_version != 1 {
        errors.push(format!(
            "architecture/modules.toml: unsupported schema_version {}",
            modules.schema_version
        ));
    }
    if capabilities.schema_version != 1 {
        errors.push(format!(
            "architecture/capabilities.toml: unsupported schema_version {}",
            capabilities.schema_version
        ));
    }
    if changes.schema_version != 1 {
        errors.push(format!(
            "architecture/contract-changes.toml: unsupported schema_version {}",
            changes.schema_version
        ));
    }

    let mut id_to_module: HashMap<&str, &ModuleRecord> = HashMap::new();
    let mut crate_to_module: HashMap<&str, &ModuleRecord> = HashMap::new();
    let mut prefix_owners: Vec<(String, String)> = Vec::new();

    for p in &modules.tooling.owned {
        prefix_owners.push((normalize_path(p), "tooling".to_string()));
    }

    for m in &modules.modules {
        if id_to_module.insert(&m.id, m).is_some() {
            errors.push(format!("duplicate module id `{}` in modules.toml", m.id));
        }
        if crate_to_module.insert(&m.crate_name, m).is_some() {
            errors.push(format!(
                "duplicate crate `{}` in modules.toml",
                m.crate_name
            ));
        }
        if m.optional && m.state == ModuleState::Active && m.owned.is_empty() {
            errors.push(format!(
                "optional active module `{}` has no owned paths",
                m.id
            ));
        }
        if m.owned.is_empty() {
            errors.push(format!("module `{}` has empty `owned` list", m.id));
        }
        for p in &m.owned {
            prefix_owners.push((normalize_path(p), format!("module:{}", m.id)));
        }
    }

    // Check duplicate or overlapping owned prefixes.
    for i in 0..prefix_owners.len() {
        for j in (i + 1)..prefix_owners.len() {
            let (p1, o1) = &prefix_owners[i];
            let (p2, o2) = &prefix_owners[j];
            if p1 == p2 || prefix_overlaps(p1, p2) || prefix_overlaps(p2, p1) {
                errors.push(format!(
                    "overlapping owned path prefixes: `{p1}` ({o1}) and `{p2}` ({o2})"
                ));
            }
        }
    }

    // Check every workspace crate (excluding xtask tooling) is registered as an active module.
    for pkg_name in cargo_pkgs.keys() {
        if pkg_name == "xtask" {
            continue;
        }
        match crate_to_module.get(pkg_name.as_str()) {
            None => errors.push(format!(
                "workspace crate `{pkg_name}` is not registered in architecture/modules.toml"
            )),
            Some(m) if m.state != ModuleState::Active => errors.push(format!(
                "workspace crate `{pkg_name}` is active in Cargo workspace but marked planned in modules.toml"
            )),
            Some(_) => {}
        }
    }

    // Check active modules exist in Cargo metadata and on disk, and verify dependencies.
    let internal_crates: HashSet<&str> = modules
        .modules
        .iter()
        .map(|m| m.crate_name.as_str())
        .collect();

    for m in &modules.modules {
        for dep in &m.production_dependencies {
            if !internal_crates.contains(dep.as_str()) {
                errors.push(format!(
                    "module `{}` lists unknown internal dependency `{dep}`",
                    m.id
                ));
            } else if m.state == ModuleState::Active {
                if let Some(target) = crate_to_module.get(dep.as_str()) {
                    if target.state == ModuleState::Planned {
                        errors.push(format!(
                            "active module `{}` cannot depend on planned module `{}` (`{dep}`)",
                            m.id, target.id
                        ));
                    }
                }
            }
        }

        if m.state == ModuleState::Active {
            for owned in &m.owned {
                if !root.join(owned).exists() {
                    errors.push(format!(
                        "active module `{}` owned path `{owned}` does not exist",
                        m.id
                    ));
                }
            }
            for cf in &m.contract_files {
                if !root.join(cf).exists() {
                    errors.push(format!(
                        "active module `{}` contract file `{cf}` does not exist",
                        m.id
                    ));
                }
            }

            match cargo_pkgs.get(&m.crate_name) {
                None => errors.push(format!(
                    "active module `{}` crate `{}` not found in `cargo metadata`",
                    m.id, m.crate_name
                )),
                Some(pkg) => {
                    let allowed: HashSet<&str> = m
                        .production_dependencies
                        .iter()
                        .map(String::as_str)
                        .collect();
                    for dep in &pkg.normal_deps {
                        if internal_crates.contains(dep.as_str()) {
                            if !allowed.contains(dep.as_str()) {
                                errors.push(format!(
                                    "forbidden internal production dependency: `{}` -> `{dep}` (not in allowlist for module `{}`)",
                                    m.crate_name, m.id
                                ));
                            }
                            if let Some(target) = crate_to_module.get(dep.as_str()) {
                                if target.state == ModuleState::Planned {
                                    errors.push(format!(
                                        "dangling optional/planned dependency: active crate `{}` depends on planned crate `{dep}`",
                                        m.crate_name
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    // Cycle detection over declared module dependency graph.
    if let Some(cycle) = find_dependency_cycle(modules) {
        errors.push(format!("dependency cycle detected: {}", cycle.join(" -> ")));
    }

    // Validate capabilities manifest.
    let mut cap_ids: HashSet<&str> = HashSet::new();
    let mut read_bits_used: HashMap<u8, &str> = HashMap::new();
    let mut write_bits_used: HashMap<u8, &str> = HashMap::new();
    for cap in &capabilities.capabilities {
        if !cap_ids.insert(&cap.id) {
            errors.push(format!(
                "duplicate capability id `{}` in capabilities.toml",
                cap.id
            ));
        }
        if cap.interface_version == 0 {
            errors.push(format!(
                "capability `{}` must have interface_version >= 1",
                cap.id
            ));
        }
        if !cap.persistent && (!cap.read_bits.is_empty() || !cap.write_bits.is_empty()) {
            errors.push(format!(
                "non-persistent capability `{}` cannot allocate read_bits or write_bits",
                cap.id
            ));
        }
        if cap.retired && cap.read_bits.is_empty() && cap.write_bits.is_empty() {
            errors.push(format!(
                "retired capability `{}` has no reserved bits to protect",
                cap.id
            ));
        }
        if !id_to_module.contains_key(cap.owner.as_str()) {
            errors.push(format!(
                "capability `{}` references unknown owner module `{}`",
                cap.id, cap.owner
            ));
        }
        for &bit in &cap.read_bits {
            if bit >= 64 {
                errors.push(format!(
                    "capability `{}` has invalid read_bit {bit} (must be 0..=63)",
                    cap.id
                ));
            } else if let Some(prev) = read_bits_used.insert(bit, &cap.id) {
                errors.push(format!(
                    "duplicate read_bit {bit} allocated to both `{prev}` and `{}`",
                    cap.id
                ));
            }
        }
        for &bit in &cap.write_bits {
            if bit >= 64 {
                errors.push(format!(
                    "capability `{}` has invalid write_bit {bit} (must be 0..=63)",
                    cap.id
                ));
            } else if let Some(prev) = write_bits_used.insert(bit, &cap.id) {
                errors.push(format!(
                    "duplicate write_bit {bit} allocated to both `{prev}` and `{}`",
                    cap.id
                ));
            }
        }
    }

    // Validate contract change notes metadata.
    let mut note_ids = HashSet::new();
    for note in &changes.changes {
        if !note_ids.insert(&note.id) {
            errors.push(format!("duplicate contract change note id `{}`", note.id));
        }
        if note.reason.trim().is_empty() {
            errors.push(format!(
                "contract change note `{}` must have a non-empty reason",
                note.id
            ));
        }
        if note.evidence.is_empty() {
            errors.push(format!(
                "contract change note `{}` must list at least one evidence entry",
                note.id
            ));
        }
        if !matches!(
            note.compatibility.as_str(),
            "compatible" | "api_change" | "format_change"
        ) {
            errors.push(format!(
                "contract change note `{}` has invalid compatibility `{}`",
                note.id, note.compatibility
            ));
        }
        for m in &note.modules {
            if !id_to_module.contains_key(m.as_str()) {
                errors.push(format!(
                    "contract change note `{}` references unknown module `{m}`",
                    note.id
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n  - "))
    }
}

pub fn validate_change_notes(
    modules: &ModulesManifest,
    changes: &ContractChangesManifest,
    changed_paths: &[String],
    added_note_ids: &HashSet<String>,
) -> Result<(), String> {
    let mut errors = Vec::new();
    let mut protected_files: HashSet<String> = HashSet::new();
    protected_files.insert("include/oryq.h".to_string());
    protected_files.insert("architecture/capabilities.toml".to_string());
    for m in &modules.modules {
        for cf in &m.contract_files {
            protected_files.insert(normalize_path(cf));
        }
    }

    let candidate_notes: Vec<&ContractChangeRecord> = changes
        .changes
        .iter()
        .filter(|n| added_note_ids.contains(&n.id))
        .collect();

    for raw_path in changed_paths {
        validate_relative_path(raw_path)?;
        let path = normalize_path(raw_path);
        if protected_files.contains(&path) {
            let covered = candidate_notes
                .iter()
                .any(|note| note.files.iter().any(|f| normalize_path(f) == path));
            if !covered {
                errors.push(format!(
                    "protected contract file `{path}` changed without a newly added note in architecture/contract-changes.toml"
                ));
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n  - "))
    }
}

fn validate_relative_path(path_str: &str) -> Result<(), String> {
    let p = Path::new(path_str);
    if p.is_absolute() {
        return Err(format!("absolute path `{path_str}` is not allowed"));
    }
    for comp in p.components() {
        if matches!(
            comp,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        ) {
            return Err(format!("outside-root path `{path_str}` is not allowed"));
        }
    }
    Ok(())
}

fn normalize_path(p: &str) -> String {
    p.replace('\\', "/").trim_start_matches("./").to_string()
}

fn prefix_overlaps(a: &str, b: &str) -> bool {
    if a.ends_with('/') {
        b.starts_with(a)
    } else {
        false
    }
}

fn find_dependency_cycle(modules: &ModulesManifest) -> Option<Vec<String>> {
    let mut graph: HashMap<&str, Vec<&str>> = HashMap::new();
    for m in &modules.modules {
        graph.insert(
            m.crate_name.as_str(),
            m.production_dependencies
                .iter()
                .map(String::as_str)
                .collect(),
        );
    }

    let mut state: HashMap<&str, u8> = HashMap::new(); // 0=unvisited, 1=visiting, 2=done
    let mut stack: Vec<&str> = Vec::new();

    for m in &modules.modules {
        let start = m.crate_name.as_str();
        if state.get(start).copied().unwrap_or(0) == 0 {
            if let Some(cycle) = dfs_cycle(start, &graph, &mut state, &mut stack) {
                return Some(cycle);
            }
        }
    }
    None
}

fn dfs_cycle<'a>(
    node: &'a str,
    graph: &HashMap<&'a str, Vec<&'a str>>,
    state: &mut HashMap<&'a str, u8>,
    stack: &mut Vec<&'a str>,
) -> Option<Vec<String>> {
    state.insert(node, 1);
    stack.push(node);

    if let Some(neighbors) = graph.get(node) {
        for &next in neighbors {
            match state.get(next).copied().unwrap_or(0) {
                1 => {
                    let pos = stack.iter().position(|&x| x == next).unwrap_or(0);
                    let mut cycle: Vec<String> =
                        stack[pos..].iter().map(|s| (*s).to_string()).collect();
                    cycle.push(next.to_string());
                    return Some(cycle);
                }
                0 => {
                    if let Some(cycle) = dfs_cycle(next, graph, state, stack) {
                        return Some(cycle);
                    }
                }
                _ => {}
            }
        }
    }

    stack.pop();
    state.insert(node, 2);
    None
}

fn git_changed_paths(root: &Path, base_ref: &str) -> Result<Vec<String>, String> {
    let range = format!("{base_ref}...HEAD");
    let output = Command::new("git")
        .args(["diff", "--name-status", "-M", &range])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run git diff against `{base_ref}`: {e}"))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("git diff against `{base_ref}` failed: {stderr}"));
    }

    let mut paths = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let parts: Vec<&str> = line.split('\t').collect();
        if parts.len() >= 2 {
            paths.push(parts[1].to_string());
        }
        if parts.len() >= 3 {
            // Both sides of a rename are checked.
            paths.push(parts[2].to_string());
        }
    }
    Ok(paths)
}

fn git_added_change_note_ids(
    root: &Path,
    base_ref: &str,
    current: &ContractChangesManifest,
) -> Result<HashSet<String>, String> {
    let spec = format!("{base_ref}:architecture/contract-changes.toml");
    let output = Command::new("git")
        .args(["show", &spec])
        .current_dir(root)
        .output()
        .map_err(|e| format!("failed to run git show `{spec}`: {e}"))?;

    let base_ids: HashSet<String> = if output.status.success() {
        let raw = String::from_utf8_lossy(&output.stdout);
        let parsed: ContractChangesManifest = toml::from_str(&raw)
            .map_err(|e| format!("failed to parse base contract-changes.toml: {e}"))?;
        parsed.changes.into_iter().map(|c| c.id).collect()
    } else {
        HashSet::new()
    };

    Ok(current
        .changes
        .iter()
        .filter(|c| !base_ids.contains(&c.id))
        .map(|c| c.id.clone())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn sample_manifests() -> (
        ModulesManifest,
        CapabilitiesManifest,
        ContractChangesManifest,
        BTreeMap<String, CargoPackageInfo>,
    ) {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let (m, c, n) = load_manifests(&root).expect("load real manifests");
        let mut pkgs = BTreeMap::new();
        for rec in &m.modules {
            if rec.state == ModuleState::Active {
                pkgs.insert(
                    rec.crate_name.clone(),
                    CargoPackageInfo {
                        normal_deps: rec.production_dependencies.iter().cloned().collect(),
                    },
                );
            }
        }
        (m, c, n, pkgs)
    }

    fn workspace_root() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf()
    }

    #[test]
    fn self_test_rejects_forbidden_edge() {
        let (m, c, n, mut pkgs) = sample_manifests();
        pkgs.get_mut("oryq-engine")
            .unwrap()
            .normal_deps
            .insert("oryq-document".to_string());
        let err = validate_static(&workspace_root(), &m, &c, &n, &pkgs).unwrap_err();
        assert!(err.contains("forbidden internal production dependency"));
    }

    #[test]
    fn self_test_rejects_dependency_cycle() {
        let (mut m, c, n, pkgs) = sample_manifests();
        m.modules
            .iter_mut()
            .find(|x| x.id == "types")
            .unwrap()
            .production_dependencies
            .push("oryq-document".to_string());
        let err = validate_static(&workspace_root(), &m, &c, &n, &pkgs).unwrap_err();
        assert!(err.contains("dependency cycle detected"));
    }

    #[test]
    fn self_test_rejects_duplicate_owner() {
        let (mut m, c, n, pkgs) = sample_manifests();
        m.modules
            .iter_mut()
            .find(|x| x.id == "engine")
            .unwrap()
            .owned
            .push("crates/oryq-types/".to_string());
        let err = validate_static(&workspace_root(), &m, &c, &n, &pkgs).unwrap_err();
        assert!(err.contains("overlapping owned path prefixes"));
    }

    #[test]
    fn self_test_rejects_dangling_planned_optional_dependency() {
        let (mut m, c, n, mut pkgs) = sample_manifests();
        m.modules.iter_mut().find(|x| x.id == "vec").unwrap().state = ModuleState::Planned;
        m.modules
            .iter_mut()
            .find(|x| x.id == "core")
            .unwrap()
            .production_dependencies
            .push("oryq-vec".to_string());
        pkgs.get_mut("oryq-core")
            .unwrap()
            .normal_deps
            .insert("oryq-vec".to_string());
        let err = validate_static(&workspace_root(), &m, &c, &n, &pkgs).unwrap_err();
        assert!(err.contains("cannot depend on planned module"));
    }

    #[test]
    fn self_test_rejects_invalid_and_duplicate_bit_allocation() {
        let (m, mut c, n, pkgs) = sample_manifests();
        c.capabilities[0].write_bits.push(64);
        let err = validate_static(&workspace_root(), &m, &c, &n, &pkgs).unwrap_err();
        assert!(err.contains("invalid write_bit 64"));

        let (m2, mut c2, n2, pkgs2) = sample_manifests();
        c2.capabilities[0].write_bits.push(0); // collides with fts write_bit 0
        let err2 = validate_static(&workspace_root(), &m2, &c2, &n2, &pkgs2).unwrap_err();
        assert!(err2.contains("duplicate write_bit 0"));
    }

    #[test]
    fn self_test_rejects_renamed_contract_file_and_missing_note() {
        let (m, _c, n, _pkgs) = sample_manifests();
        let changed = vec![
            "crates/oryq-types/src/ids.rs".to_string(),
            "crates/oryq-types/src/ids_v2.rs".to_string(),
        ];
        let added_notes = HashSet::new();
        let err = validate_change_notes(&m, &n, &changed, &added_notes).unwrap_err();
        assert!(err.contains("crates/oryq-types/src/ids.rs"));

        let outside = vec!["../escape.rs".to_string()];
        let err2 = validate_change_notes(&m, &n, &outside, &added_notes).unwrap_err();
        assert!(err2.contains("outside-root path"));
    }
}
