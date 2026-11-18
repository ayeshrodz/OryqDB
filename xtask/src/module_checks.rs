//! Module-scoped test and reverse-consumer check runner (`cargo run -p xtask -- modules <id>`).

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
use std::process::Command;

use crate::architecture::{
    load_cargo_packages, load_manifests, validate_static, ModuleRecord, ModuleState,
};

pub fn run_module_checks(root: &Path, module_id: &str) -> Result<(), String> {
    let (modules, capabilities, changes) = load_manifests(root)?;
    let cargo_pkgs = load_cargo_packages(root)?;
    validate_static(root, &modules, &capabilities, &changes, &cargo_pkgs)?;

    let target = modules
        .modules
        .iter()
        .find(|m| m.id == module_id)
        .ok_or_else(|| format!("unknown module id `{module_id}`"))?;

    if target.state == ModuleState::Planned {
        return Err(format!(
            "module `{module_id}` (`{}`) is planned and not yet implemented",
            target.crate_name
        ));
    }

    // Run owner test commands strictly as argv arrays (no shell interpolation).
    for argv in &target.test_commands {
        if argv.is_empty() {
            return Err(format!("module `{module_id}` has an empty test command"));
        }
        println!("xtask modules [{module_id}]: running {:?}", argv);
        let status = Command::new(&argv[0])
            .args(&argv[1..])
            .current_dir(root)
            .status()
            .map_err(|e| format!("failed to execute {:?}: {e}", argv))?;
        if !status.success() {
            return Err(format!("command {:?} failed with status {status}", argv));
        }
    }

    // Check every active reverse-transitive production consumer in topological order.
    let consumers = reverse_transitive_consumers(&modules.modules, &target.crate_name);
    for crate_name in &consumers {
        let argv = ["cargo", "check", "-p", crate_name.as_str()];
        println!("xtask modules [{module_id}]: checking downstream consumer `{crate_name}`");
        let status = Command::new(argv[0])
            .args(&argv[1..])
            .current_dir(root)
            .status()
            .map_err(|e| format!("failed to execute {:?}: {e}", argv))?;
        if !status.success() {
            return Err(format!(
                "downstream consumer check `{crate_name}` failed with status {status}"
            ));
        }
    }

    println!(
        "xtask modules [{module_id}]: owner tests and {} downstream consumer check(s) passed.",
        consumers.len()
    );
    Ok(())
}

fn reverse_transitive_consumers(modules: &[ModuleRecord], target_crate: &str) -> Vec<String> {
    let active: HashMap<&str, &ModuleRecord> = modules
        .iter()
        .filter(|m| m.state == ModuleState::Active)
        .map(|m| (m.crate_name.as_str(), m))
        .collect();

    let mut rev_adj: HashMap<&str, Vec<&str>> = HashMap::new();
    for m in active.values() {
        for dep in &m.production_dependencies {
            rev_adj
                .entry(dep.as_str())
                .or_default()
                .push(m.crate_name.as_str());
        }
    }

    // BFS to collect all reachable reverse consumers.
    let mut visited: HashSet<&str> = HashSet::new();
    let mut queue = VecDeque::new();
    queue.push_back(target_crate);
    while let Some(curr) = queue.pop_front() {
        if let Some(parents) = rev_adj.get(curr) {
            for &p in parents {
                if visited.insert(p) {
                    queue.push_back(p);
                }
            }
        }
    }

    // Kahn's topological sort over visited consumers.
    let mut in_degree: HashMap<&str, usize> = HashMap::new();
    for &c in &visited {
        let deg = active[c]
            .production_dependencies
            .iter()
            .filter(|d| visited.contains(d.as_str()))
            .count();
        in_degree.insert(c, deg);
    }

    let mut ready: Vec<&str> = in_degree
        .iter()
        .filter(|&(_, &deg)| deg == 0)
        .map(|(&k, _)| k)
        .collect();
    ready.sort_unstable();

    let mut ordered = Vec::new();
    while !ready.is_empty() {
        let next = ready.remove(0);
        ordered.push(next.to_string());
        if let Some(parents) = rev_adj.get(next) {
            for &p in parents {
                if let Some(d) = in_degree.get_mut(p) {
                    *d -= 1;
                    if *d == 0 {
                        ready.push(p);
                        ready.sort_unstable();
                    }
                }
            }
        }
    }

    ordered
}
