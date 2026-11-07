//! Architecture fitness functions and ADR traceability.
//!
//! Each test enforces one or more decisions of an accepted ADR. The
//! `// @adr-NNNN-dN` comment above a test is what the traceability test
//! (ADR-0001 D15) looks for, so keep it when moving tests around.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cargo_metadata::{DependencyKind, Metadata, MetadataCommand, Package};
use regex::Regex;

// ---------------------------------------------------------------------------
// helpers

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn metadata() -> &'static Metadata {
    static META: OnceLock<Metadata> = OnceLock::new();
    META.get_or_init(|| {
        MetadataCommand::new()
            .manifest_path(workspace_root().join("Cargo.toml"))
            .no_deps()
            .exec()
            .expect("cargo metadata")
    })
}

fn members() -> Vec<&'static Package> {
    metadata().workspace_packages()
}

fn package(name: &str) -> &'static Package {
    members()
        .into_iter()
        .find(|p| p.name.as_str() == name)
        .unwrap_or_else(|| panic!("workspace crate `{name}` is missing"))
}

/// Direct dependencies of `pkg`, of any kind (normal, dev, build).
fn deps(pkg: &Package) -> BTreeSet<String> {
    pkg.dependencies.iter().map(|d| d.name.clone()).collect()
}

fn normal_deps(pkg: &Package) -> BTreeSet<String> {
    pkg.dependencies
        .iter()
        .filter(|d| d.kind == DependencyKind::Normal)
        .map(|d| d.name.clone())
        .collect()
}

/// Workspace crates that directly depend on `dep`.
fn dependents_of(dep: &str) -> BTreeSet<String> {
    members()
        .into_iter()
        .filter(|p| deps(p).contains(dep))
        .map(|p| p.name.to_string())
        .collect()
}

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn read(path: impl AsRef<Path>) -> String {
    let path = path.as_ref();
    fs::read_to_string(path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
}

// ---------------------------------------------------------------------------
// D1: Rust workspace that ships one binary

// @adr-0001-d1
#[test]
fn workspace_has_the_crates_of_adr_0001() {
    let actual: BTreeSet<String> = members().iter().map(|p| p.name.to_string()).collect();
    let expected = set(&[
        "soqrates-cli",
        "soqrates-core",
        "soqrates-examiner",
        "soqrates-mcp",
        "soqrates-scm-github",
        "soqrates-spec",
        "soqrates-store",
    ]);
    let missing: Vec<_> = expected.difference(&actual).collect();
    assert!(
        missing.is_empty(),
        "ADR-0001 D1: missing crates {missing:?}"
    );
}

// @adr-0001-d1
#[test]
fn the_only_binary_is_soqrates() {
    let bins: Vec<(String, String)> = members()
        .iter()
        .flat_map(|p| {
            p.targets
                .iter()
                .filter(|t| t.is_bin())
                .map(|t| (p.name.to_string(), t.name.clone()))
        })
        .collect();
    assert_eq!(
        bins,
        vec![("soqrates-cli".to_string(), "soqrates".to_string())],
        "ADR-0001 D1: exactly one binary, `soqrates`, in soqrates-cli"
    );
}

// ---------------------------------------------------------------------------
// D2: hexagonal core

// @adr-0001-d2
#[test]
fn core_depends_only_on_its_allowlist() {
    let core = package("soqrates-core");
    let allowed = set(&["rs-fsrs", "chrono", "serde", "serde_json", "thiserror"]);
    let extra: Vec<_> = normal_deps(core).difference(&allowed).cloned().collect();
    assert!(
        extra.is_empty(),
        "ADR-0001 D2: soqrates-core may only depend on {allowed:?}, found {extra:?}"
    );
}

// @adr-0001-d2
#[test]
fn core_depends_on_no_workspace_crate() {
    let workspace: BTreeSet<String> = members().iter().map(|p| p.name.to_string()).collect();
    let bad: Vec<_> = deps(package("soqrates-core"))
        .intersection(&workspace)
        .cloned()
        .collect();
    assert!(
        bad.is_empty(),
        "ADR-0001 D2: soqrates-core depends on {bad:?}"
    );
}

// @adr-0001-d2 @adr-0001-d4 @adr-0001-d5
#[test]
fn core_defines_the_ports() {
    let ports = read(workspace_root().join("crates/soqrates-core/src/ports.rs"));
    for port in ["StateStore", "Examiner", "ScmProvider", "Clock", "Prompter"] {
        assert!(
            ports.contains(&format!("pub trait {port}")),
            "ADR-0001 D2: port `{port}` must be defined in soqrates-core/src/ports.rs"
        );
    }
}

// @adr-0001-d2
#[test]
fn adapters_depend_on_core() {
    for adapter in [
        "soqrates-store",
        "soqrates-examiner",
        "soqrates-scm-github",
        "soqrates-mcp",
    ] {
        assert!(
            normal_deps(package(adapter)).contains("soqrates-core"),
            "ADR-0001 D2: adapter `{adapter}` must implement ports from soqrates-core"
        );
    }
}

// ---------------------------------------------------------------------------
// D3, D4, D10: third-party frameworks are confined to one adapter each

fn assert_confined(dep: &str, owner: &str, decision: &str) {
    assert_eq!(
        dependents_of(dep),
        set(&[owner]),
        "ADR-0001 {decision}: `{dep}` may only (and must) be a dependency of `{owner}`"
    );
}

// @adr-0001-d3
#[test]
fn rig_is_confined_to_soqrates_examiner() {
    assert_confined("rig-core", "soqrates-examiner", "D3");
}

// @adr-0001-d4
#[test]
fn octocrab_is_confined_to_soqrates_scm_github() {
    assert_confined("octocrab", "soqrates-scm-github", "D4");
}

// @adr-0001-d10
#[test]
fn rmcp_is_confined_to_soqrates_mcp() {
    assert_confined("rmcp", "soqrates-mcp", "D10");
}

// @adr-0001-d5
#[test]
fn store_adapters_live_in_soqrates_store() {
    let store = read(workspace_root().join("crates/soqrates-store/src/lib.rs"));
    for adapter in ["InMemoryStore", "FileStore"] {
        assert!(
            store.contains(&format!("impl StateStore for {adapter}")),
            "ADR-0001 D5: soqrates-store must provide `{adapter}`"
        );
    }
}

// ---------------------------------------------------------------------------
// D6: FSRS via rs-fsrs

// @adr-0001-d6
#[test]
fn scheduling_uses_rs_fsrs_without_the_training_crate() {
    assert_confined("rs-fsrs", "soqrates-core", "D6");
    assert!(
        dependents_of("fsrs").is_empty(),
        "ADR-0001 D6: use the scheduler-only `rs-fsrs`, not `fsrs` (pulls in an ML training stack)"
    );
}

// ---------------------------------------------------------------------------
// D11: skills

// @adr-0001-d11
#[test]
fn skills_have_valid_frontmatter() {
    let dir = workspace_root().join("skills");
    let mut count = 0;
    for entry in fs::read_dir(&dir).expect("skills/ directory (ADR-0001 D11)") {
        let entry = entry.unwrap();
        if !entry.file_type().unwrap().is_dir() {
            continue;
        }
        count += 1;
        let dir_name = entry.file_name().to_string_lossy().to_string();
        let text = read(entry.path().join("SKILL.md"));
        let front = text
            .strip_prefix("---\n")
            .and_then(|rest| rest.split_once("\n---\n"))
            .map(|(front, _)| front)
            .unwrap_or_else(|| {
                panic!("ADR-0001 D11: skills/{dir_name}/SKILL.md needs YAML frontmatter")
            });
        let field = |key: &str| {
            front
                .lines()
                .find_map(|l| l.strip_prefix(&format!("{key}:")))
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        assert_eq!(
            field("name").as_deref(),
            Some(dir_name.as_str()),
            "ADR-0001 D11: skills/{dir_name}: `name` must equal the directory name"
        );
        assert!(
            field("description").is_some(),
            "ADR-0001 D11: skills/{dir_name}: `description` is required"
        );
    }
    assert!(count > 0, "ADR-0001 D11: expected at least one skill");
}

// ---------------------------------------------------------------------------
// D12: hermetic tests

// @adr-0001-d12
#[test]
fn spec_does_not_depend_on_networked_adapters() {
    let spec = deps(package("soqrates-spec"));
    for adapter in [
        "soqrates-examiner",
        "soqrates-scm-github",
        "soqrates-cli",
        "rig-core",
        "octocrab",
    ] {
        assert!(
            !spec.contains(adapter),
            "ADR-0001 D12: soqrates-spec must not depend on `{adapter}`"
        );
    }
}

// ---------------------------------------------------------------------------
// D15: ADRs are executable (traceability)

#[derive(Debug)]
struct Decision {
    adr: String,
    id: String,
    title: String,
    automated: bool,
    reason: String,
}

impl Decision {
    fn tag(&self) -> String {
        format!("adr-{}-{}", self.adr, self.id.to_lowercase())
    }
}

/// All decisions of accepted ADRs in docs/adr.
fn decisions() -> Vec<Decision> {
    let heading = Regex::new(r"^### (D\d+): (.+)$").unwrap();
    let enforcement = Regex::new(r"^\*\*Enforcement:\*\* (automated|manual)(.*)$").unwrap();
    let file_name = Regex::new(r"^(\d{4})-.*\.md$").unwrap();

    let mut out = Vec::new();
    let mut files: Vec<_> = fs::read_dir(workspace_root().join("docs/adr"))
        .expect("docs/adr")
        .map(|e| e.unwrap().path())
        .collect();
    files.sort();
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Some(num) = file_name.captures(&name).map(|c| c[1].to_string()) else {
            continue;
        };
        let text = read(&path);
        if !text.contains("**Status:** Accepted") {
            continue;
        }
        let mut current: Option<(String, String)> = None;
        for line in text.lines() {
            if let Some(c) = heading.captures(line) {
                if let Some((id, _)) = &current {
                    panic!("ADR-{num} {id}: missing **Enforcement:** line");
                }
                current = Some((c[1].to_string(), c[2].to_string()));
            } else if let Some(c) = enforcement.captures(line) {
                let (id, title) = current
                    .take()
                    .unwrap_or_else(|| panic!("ADR-{num}: **Enforcement:** outside a decision"));
                out.push(Decision {
                    adr: num.clone(),
                    id,
                    title,
                    automated: &c[1] == "automated",
                    reason: c[2].trim_start_matches(['.', ' ']).trim().to_string(),
                });
            }
        }
        if let Some((id, _)) = current {
            panic!("ADR-{num} {id}: missing **Enforcement:** line");
        }
    }
    out
}

/// Tag -> places that reference it, from feature files and test comments.
fn references() -> BTreeMap<String, Vec<String>> {
    let spec = Path::new(env!("CARGO_MANIFEST_DIR"));
    let gherkin_tag = Regex::new(r"@(adr-\d{4}-d\d+)\b").unwrap();
    let rust_comment = Regex::new(r"^\s*//.*").unwrap();
    let mut refs: BTreeMap<String, Vec<String>> = BTreeMap::new();

    let mut visit = |path: &Path, only_comments: bool| {
        let text = read(path);
        let rel = path
            .strip_prefix(spec)
            .unwrap_or(path)
            .display()
            .to_string();
        for (n, line) in text.lines().enumerate() {
            if only_comments && !rust_comment.is_match(line) {
                continue;
            }
            if !only_comments && !line.trim_start().starts_with('@') {
                continue;
            }
            for c in gherkin_tag.captures_iter(line) {
                refs.entry(c[1].to_string())
                    .or_default()
                    .push(format!("{rel}:{}", n + 1));
            }
        }
    };

    fn walk(dir: &Path, f: &mut dyn FnMut(&Path)) {
        let Ok(entries) = fs::read_dir(dir) else {
            return;
        };
        for e in entries {
            let p = e.unwrap().path();
            if p.is_dir() {
                walk(&p, f);
            } else {
                f(&p);
            }
        }
    }
    walk(&spec.join("features"), &mut |p| {
        if p.extension().is_some_and(|e| e == "feature") {
            visit(p, false)
        }
    });
    walk(&spec.join("tests"), &mut |p| {
        if p.extension().is_some_and(|e| e == "rs") {
            visit(p, true)
        }
    });
    refs
}

// @adr-0001-d15
#[test]
fn every_automated_decision_is_enforced_by_a_test() {
    let decisions = decisions();
    assert!(!decisions.is_empty(), "no accepted ADR decisions found");
    let refs = references();

    let mut report = String::from("\nADR traceability:\n");
    let mut unenforced = Vec::new();
    for d in &decisions {
        let places = refs.get(&d.tag()).map(Vec::len).unwrap_or(0);
        let status = match (d.automated, places) {
            (true, 0) => {
                unenforced.push(format!("ADR-{} {}: {}", d.adr, d.id, d.title));
                "MISSING".to_string()
            }
            (true, n) => format!("{n} test(s)"),
            (false, _) => format!("manual: {}", d.reason),
        };
        report.push_str(&format!(
            "  ADR-{} {:<4} {:<55} {status}\n",
            d.adr, d.id, d.title
        ));
    }
    println!("{report}");
    assert!(
        unenforced.is_empty(),
        "ADR-0001 D15: automated decisions without any tagged test:\n  {}",
        unenforced.join("\n  ")
    );
}

// @adr-0001-d15
#[test]
fn manual_decisions_say_why() {
    for d in decisions().iter().filter(|d| !d.automated) {
        assert!(
            !d.reason.is_empty(),
            "ADR-{} {}: a manual decision must say why it is not automated (\"**Enforcement:** manual. <why>\")",
            d.adr,
            d.id
        );
    }
}

// @adr-0001-d15
#[test]
fn tests_only_reference_existing_decisions() {
    let known: BTreeSet<String> = decisions().iter().map(Decision::tag).collect();
    let dangling: Vec<String> = references()
        .into_iter()
        .filter(|(tag, _)| !known.contains(tag))
        .map(|(tag, places)| format!("@{tag} at {}", places.join(", ")))
        .collect();
    assert!(
        dangling.is_empty(),
        "tags referencing unknown ADR decisions:\n  {}",
        dangling.join("\n  ")
    );
}
