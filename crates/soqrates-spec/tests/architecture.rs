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

// ===========================================================================
// ADR-0001
// ===========================================================================

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
    let ports = read(workspace_root().join("crates/soqrates-core/src/app/ports.rs"));
    for port in ["StateStore", "Examiner", "ScmProvider", "Clock", "Prompter"] {
        assert!(
            ports.contains(&format!("pub trait {port}")),
            "ADR-0001 D2: port `{port}` must be defined in soqrates-core/src/app/ports.rs"
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
        "{decision}: `{dep}` may only (and must) be a dependency of `{owner}`"
    );
}

// @adr-0001-d3
#[test]
fn rig_is_confined_to_soqrates_examiner() {
    assert_confined("rig-core", "soqrates-examiner", "ADR-0001 D3");
}

// @adr-0001-d4
#[test]
fn octocrab_is_confined_to_soqrates_scm_github() {
    assert_confined("octocrab", "soqrates-scm-github", "ADR-0001 D4");
}

// @adr-0001-d10
#[test]
fn rmcp_is_confined_to_soqrates_mcp() {
    assert_confined("rmcp", "soqrates-mcp", "ADR-0001 D10");
}

// @adr-0001-d5 @adr-0005-d1
#[test]
fn store_adapters_live_in_soqrates_store() {
    let dir = workspace_root().join("crates/soqrates-store/src");
    let store = read(dir.join("lib.rs")) + &read(dir.join("sql.rs"));
    for adapter in ["InMemoryStore", "FileStore", "SqlStore"] {
        assert!(
            store.contains(&format!("impl StateStore for {adapter}")),
            "ADR-0001 D5, ADR-0005 D1: soqrates-store must provide `{adapter}`"
        );
    }
}

// @adr-0005-d1
#[test]
fn sqlx_is_confined_to_soqrates_store() {
    assert_confined("sqlx", "soqrates-store", "ADR-0005 D1");
}

// @adr-0005-d1 @adr-0005-d6
#[test]
fn database_backends_are_features_and_spec_uses_sqlite_only() {
    let store = package("soqrates-store");
    for feature in ["sqlite", "postgres"] {
        assert!(
            store.features.contains_key(feature),
            "ADR-0005 D1: soqrates-store must have a `{feature}` feature"
        );
    }
    let sqlx = store
        .dependencies
        .iter()
        .find(|d| d.name == "sqlx")
        .expect("soqrates-store depends on sqlx");
    assert!(sqlx.optional, "ADR-0005 D1: sqlx must be optional");
    let spec_store = package("soqrates-spec")
        .dependencies
        .iter()
        .find(|d| d.name == "soqrates-store")
        .expect("soqrates-spec depends on soqrates-store");
    assert_eq!(
        spec_store.features,
        ["sqlite"],
        "ADR-0005 D6: soqrates-spec builds the SQLite backend only"
    );
}

// ---------------------------------------------------------------------------
// D6: FSRS via rs-fsrs

// @adr-0001-d6
#[test]
fn scheduling_uses_rs_fsrs_without_the_training_crate() {
    assert_confined("rs-fsrs", "soqrates-core", "ADR-0001 D6");
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

// ===========================================================================
// ADR-0006: distribution as a GitHub Action
// ===========================================================================

fn action_yml() -> String {
    read(workspace_root().join("action.yml"))
}

/// The `with:`-able inputs of `action.yml`: the keys indented by two spaces
/// under `inputs:`.
fn action_inputs(action: &str) -> BTreeSet<String> {
    let block = action
        .split_once("\ninputs:\n")
        .expect("action.yml declares inputs")
        .1;
    let key = Regex::new(r"^  ([a-z][a-z0-9-]*):\s*$").unwrap();
    block
        .lines()
        .take_while(|l| l.is_empty() || l.starts_with(' ') || l.starts_with('#'))
        .filter_map(|l| key.captures(l).map(|c| c[1].to_string()))
        .collect()
}

// @adr-0006-d1
#[test]
fn soqrates_ships_as_a_composite_action_that_never_checks_out_code() {
    let action = action_yml();
    assert!(
        action.contains("using: composite"),
        "ADR-0006 D1: action.yml is a composite action"
    );
    assert!(
        action.contains("\"$SOQRATES\" \"${args[@]}\"") && action.contains("args=(ci "),
        "ADR-0006 D1: the action runs `soqrates ci`"
    );
    for file in [
        "action.yml",
        "action/install.sh",
        "action/state-checkout.sh",
    ] {
        let text = read(workspace_root().join(file));
        assert!(
            !text.contains("actions/checkout"),
            "ADR-0006 D1: {file} must not check out the user's code"
        );
    }
    let guide = read(workspace_root().join("docs/using-the-action.md"));
    for (needle, why) in [
        ("uses: jvanbuel/soqrates@v", "calls the released action"),
        ("pull_request:", "declares the pull_request trigger"),
        ("issue_comment:", "declares the comment trigger"),
        ("- [x] **Grade my answers**", "filters comment edits"),
        ("cancel-in-progress: false", "serialises runs"),
    ] {
        assert!(
            guide.contains(needle),
            "ADR-0006 D1: the workflow in the guide {why} (`{needle}`)"
        );
    }
}

// @adr-0006-d2
#[test]
fn the_action_runs_a_verified_release_binary_and_never_compiles() {
    let install = read(workspace_root().join("action/install.sh"));
    for (needle, why) in [
        ("gh release download", "downloads the release archive"),
        ("--check", "verifies the SHA-256 checksum"),
        ("gh attestation verify", "verifies the build provenance"),
        ("Cargo.toml", "picks the release of its own version"),
    ] {
        assert!(
            install.contains(needle),
            "ADR-0006 D2: install.sh {why} (`{needle}`)"
        );
    }
    for file in ["action.yml", "action/install.sh"] {
        assert!(
            !read(workspace_root().join(file)).contains("cargo "),
            "ADR-0006 D2: {file} must never compile"
        );
    }
    let release = read(workspace_root().join(".github/workflows/release.yml"));
    for target in [
        "x86_64-unknown-linux-gnu",
        "aarch64-unknown-linux-gnu",
        "aarch64-apple-darwin",
    ] {
        assert!(
            release.contains(&format!("target: {target}")),
            "ADR-0006 D2: the release builds {target}"
        );
        assert!(
            install.contains(&format!("target={target}")),
            "ADR-0006 D2: the action can install {target}"
        );
    }
    for (needle, why) in [
        ("shasum -a 256", "publishes SHA-256 checksums"),
        (
            "actions/attest-build-provenance",
            "attests build provenance",
        ),
    ] {
        assert!(
            release.contains(needle),
            "ADR-0006 D2: the release workflow {why}"
        );
    }
}

// @adr-0006-d3
#[test]
fn users_bring_their_own_credentials_and_persistent_state() {
    let action = action_yml();
    let inputs = action_inputs(&action);
    for input in [
        "model",
        "anthropic-api-key",
        "openai-api-key",
        "ollama-base-url",
        "ollama-api-key",
        "mode",
        "database-url",
        "state-branch",
    ] {
        assert!(
            inputs.contains(input),
            "ADR-0006 D3: action.yml declares the input `{input}`, got {inputs:?}"
        );
    }
    // Each credential reaches the examiner under the name it reads.
    for (var, input) in [
        ("ANTHROPIC_API_KEY", "anthropic-api-key"),
        ("OPENAI_API_KEY", "openai-api-key"),
        ("OLLAMA_API_BASE_URL", "ollama-base-url"),
        ("OLLAMA_API_KEY", "ollama-api-key"),
        ("SOQRATES_MODEL", "model"),
        ("SOQRATES_DATABASE_URL", "database-url"),
    ] {
        assert!(
            action.contains(&format!("{var}: ${{{{ inputs.{input} }}}}")),
            "ADR-0006 D3: the action exports `{input}` as {var}"
        );
    }
    let examiner = read(workspace_root().join("crates/soqrates-examiner/src/lib.rs"));
    for var in ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "OLLAMA_API_BASE_URL"] {
        assert!(
            examiner.contains(var),
            "ADR-0006 D3: the examiner reads {var}"
        );
    }
    assert!(
        action.contains("default: advisory"),
        "ADR-0006 D3: advisory is the default mode"
    );
    assert!(
        action.contains("sqlite:*") && action.contains("github-hosted"),
        "ADR-0006 D3: a sqlite: database-url is refused on GitHub-hosted runners"
    );
}

// @adr-0006-d4
#[test]
fn comments_post_as_the_workflow_token_or_the_users_own_app() {
    let action = action_yml();
    let inputs = action_inputs(&action);
    for input in ["github-token", "app-id", "app-private-key"] {
        assert!(
            inputs.contains(input),
            "ADR-0006 D4: action.yml declares the input `{input}`"
        );
    }
    assert!(
        action.contains("default: ${{ github.token }}"),
        "ADR-0006 D4: the workflow token is the default"
    );
    assert!(
        action.contains("actions/create-github-app-token")
            && action
                .contains("GITHUB_TOKEN: ${{ steps.app.outputs.token || inputs.github-token }}"),
        "ADR-0006 D4: with an app, Soqrates posts with the app's token"
    );
}

// @adr-0006-d5
#[test]
fn releases_are_semver_tags_and_this_repository_runs_the_action_from_source() {
    let config = read(workspace_root().join("release-plz.toml"));
    for (needle, why) in [
        ("git_only = true", "reads previous versions from git tags"),
        ("publish = false", "publishes nothing to crates.io"),
        (r#"git_tag_name = "v{{ version }}""#, "tags releases vX.Y.Z"),
    ] {
        assert!(
            config.contains(needle),
            "ADR-0006 D5: release-plz.toml {why} (`{needle}`)"
        );
    }
    let release_plz = read(workspace_root().join(".github/workflows/release-plz.yml"));
    for (needle, why) in [
        ("command: release-pr", "keeps a release PR open"),
        ("command: release", "tags merged releases"),
        (
            "uses: ./.github/workflows/release.yml",
            "builds the binaries of a new release",
        ),
    ] {
        assert!(
            release_plz.contains(needle),
            "ADR-0006 D5: the release-plz workflow {why}"
        );
    }
    let release = read(workspace_root().join(".github/workflows/release.yml"));
    for (needle, why) in [
        ("workflow_call:", "is called with the new tag"),
        (
            "doesn't match the crate version",
            "checks the tag matches the crate version",
        ),
        ("Move the major tag", "moves the major tag"),
    ] {
        assert!(
            release.contains(needle),
            "ADR-0006 D5: the release workflow {why}"
        );
    }
    let workflow = read(workspace_root().join(".github/workflows/soqrates.yml"));
    assert!(
        workflow.contains("uses: ./") && workflow.contains("binary: target/release/soqrates"),
        "ADR-0006 D5: the dogfooding workflow runs the action from its checkout, with a binary built from source"
    );
}

// ===========================================================================
// ADR-0002: domain model
// ===========================================================================

fn core_src() -> PathBuf {
    workspace_root().join("crates/soqrates-core/src")
}

/// All `.rs` files under `dir`, recursively, sorted.
fn rust_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// The ADR-0002 context a core source file belongs to, or `None` for lib.rs.
fn context_of(file: &Path) -> Option<String> {
    let rel = file.strip_prefix(core_src()).unwrap();
    let first = rel
        .components()
        .next()?
        .as_os_str()
        .to_string_lossy()
        .to_string();
    match first.as_str() {
        "lib.rs" => None,
        other => Some(other.trim_end_matches(".rs").to_string()),
    }
}

/// Rust source with comments stripped (line comments and doc comments), so
/// that prose doesn't count as a dependency.
fn code_only(text: &str) -> String {
    text.lines()
        .map(|l| match l.find("//") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

struct GlossaryEntry {
    term: String,
    code: String,
    context: String,
    avoid: Vec<String>,
}

fn glossary() -> Vec<GlossaryEntry> {
    let text = read(workspace_root().join("docs/adr/0002-domain-model.md"));
    let section = text
        .split("## Glossary")
        .nth(1)
        .and_then(|s| s.split("\n## ").next())
        .expect("ADR-0002 has a ## Glossary section");
    let row =
        Regex::new(r"^\|\s*([^|]+?)\s*\|\s*`(\w+)`\s*\|\s*(\w+)\s*\|[^|]*\|\s*([^|]*?)\s*\|$")
            .unwrap();
    let entries: Vec<_> = section
        .lines()
        .filter_map(|l| row.captures(l))
        .map(|c| GlossaryEntry {
            term: c[1].to_string(),
            code: c[2].to_string(),
            context: c[3].to_string(),
            avoid: c[4]
                .split(',')
                .map(|w| w.trim().to_lowercase())
                .filter(|w| !w.is_empty())
                .collect(),
        })
        .collect();
    assert!(
        entries.len() > 5,
        "ADR-0002 glossary table could not be parsed"
    );
    entries
}

// @adr-0002-d1
#[test]
fn every_glossary_term_exists_in_its_context() {
    let files = rust_files(&core_src());
    let ctx_dir = |ctx: &str| {
        if ctx == "shared" {
            "ids".to_string()
        } else {
            ctx.to_string()
        }
    };
    let mut problems = Vec::new();
    for e in glossary() {
        let def = Regex::new(&format!(
            r"pub\s+(async\s+)?(struct|enum|trait|fn|type)\s+{}\b",
            regex::escape(&e.code)
        ))
        .unwrap();
        let found: Vec<String> = files
            .iter()
            .filter(|f| def.is_match(&code_only(&read(f))))
            .filter_map(|f| context_of(f))
            .collect();
        let home = ctx_dir(&e.context);
        // Ports are defined in the app layer, whatever their context.
        let is_port = found.iter().any(|c| c == "app")
            && read(core_src().join("app/ports.rs")).contains(&format!("pub trait {}", e.code));
        if found.is_empty() {
            problems.push(format!(
                "`{}` ({}) is not defined in soqrates-core",
                e.code, e.term
            ));
        } else if !found.contains(&home) && !is_port {
            problems.push(format!(
                "`{}` ({}) belongs to context `{}` but is defined in {:?}",
                e.code, e.term, e.context, found
            ));
        }
    }
    assert!(
        problems.is_empty(),
        "ADR-0002 D1:\n  {}",
        problems.join("\n  ")
    );
}

// @adr-0002-d1
#[test]
fn feature_files_avoid_non_glossary_synonyms() {
    let features = Path::new(env!("CARGO_MANIFEST_DIR")).join("features");
    let mut problems = Vec::new();
    for e in glossary() {
        for word in &e.avoid {
            let re = Regex::new(&format!(r"(?i)\b{}\b", regex::escape(word))).unwrap();
            for entry in fs::read_dir(&features).unwrap() {
                let path = entry.unwrap().path();
                for (n, line) in read(&path).lines().enumerate() {
                    if re.is_match(line) {
                        problems.push(format!(
                            "{}:{}: \"{word}\" (say \"{}\")",
                            path.file_name().unwrap().to_string_lossy(),
                            n + 1,
                            e.term.to_lowercase()
                        ));
                    }
                }
            }
        }
    }
    assert!(
        problems.is_empty(),
        "ADR-0002 D1:\n  {}",
        problems.join("\n  ")
    );
}

// @adr-0002-d2
#[test]
fn contexts_respect_the_dependency_direction() {
    let allowed: BTreeMap<&str, &[&str]> = BTreeMap::from([
        ("ids", &[][..]),
        ("assessment", &["ids"][..]),
        ("learner", &["ids", "assessment"][..]),
        ("delivery", &["ids", "assessment", "learner"][..]),
        (
            "app",
            &["ids", "assessment", "learner", "delivery", "app"][..],
        ),
    ]);
    // Items defined in lib.rs itself are usable by everyone.
    let root_items = ["Result", "Error"];
    let path = Regex::new(r"crate::(\w+)").unwrap();
    let group = Regex::new(r"crate::\{([^}]*)\}").unwrap();

    let mut problems = Vec::new();
    for file in rust_files(&core_src()) {
        let Some(ctx) = context_of(&file) else {
            continue;
        };
        let Some(ok) = allowed.get(ctx.as_str()) else {
            problems.push(format!(
                "{}: module `{ctx}` is not an ADR-0002 context",
                file.display()
            ));
            continue;
        };
        let code = code_only(&read(&file));
        let mut used: BTreeSet<String> = path
            .captures_iter(&code)
            .map(|c| c[1].to_string())
            .collect();
        for g in group.captures_iter(&code) {
            used.extend(
                g[1].split(',')
                    .filter_map(|p| p.trim().split("::").next())
                    .map(str::to_string),
            );
        }
        for u in used {
            if u != ctx && !ok.contains(&u.as_str()) && !root_items.contains(&u.as_str()) {
                let rel = file.strip_prefix(core_src()).unwrap().display();
                problems.push(format!("{rel}: `{ctx}` must not use `{u}`"));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "ADR-0002 D2:\n  {}",
        problems.join("\n  ")
    );
}

// @adr-0002-d2
#[test]
fn domain_contexts_are_synchronous() {
    let asynchronous = Regex::new(r"\basync\b|\bFuture\b|\.await\b").unwrap();
    let mut problems = Vec::new();
    for file in rust_files(&core_src()) {
        let Some(ctx) = context_of(&file) else {
            continue;
        };
        if ctx == "app" {
            continue;
        }
        for (n, line) in code_only(&read(&file)).lines().enumerate() {
            if asynchronous.is_match(line) {
                let rel = file.strip_prefix(core_src()).unwrap().display();
                problems.push(format!("{rel}:{}: {}", n + 1, line.trim()));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "ADR-0002 D2: domain contexts must be synchronous:\n  {}",
        problems.join("\n  ")
    );
}

/// The text between the braces following the first match of `header`.
fn braced_block<'a>(text: &'a str, header: &str) -> &'a str {
    let start = text
        .find(header)
        .unwrap_or_else(|| panic!("`{header}` not found"));
    let open = start + text[start..].find('{').unwrap();
    let mut depth = 0;
    for (i, ch) in text[open..].char_indices() {
        match ch {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return &text[open + 1..open + i];
                }
            }
            _ => {}
        }
    }
    panic!("unbalanced braces after `{header}`")
}

// @adr-0002-d3
#[test]
fn aggregates_protect_their_state() {
    // (aggregate, file, pub methods allowed to take `&mut self`)
    let aggregates = [
        ("LearnerProfile", "learner/mod.rs", &["record"][..]),
        ("Session", "delivery/mod.rs", &[][..]),
    ];
    let pub_field = Regex::new(r"^\s*pub(\([^)]*\))?\s+\w+\s*:").unwrap();
    let mut_method = Regex::new(r"pub\s+fn\s+(\w+)[^{]*&mut\s+self").unwrap();
    let mut problems = Vec::new();
    for (name, file, mutators) in aggregates {
        let code = code_only(&read(core_src().join(file)));
        for line in braced_block(&code, &format!("pub struct {name} ")).lines() {
            if pub_field.is_match(line) {
                problems.push(format!("{name}: public field `{}`", line.trim()));
            }
        }
        for c in mut_method.captures_iter(braced_block(&code, &format!("impl {name} "))) {
            if !mutators.contains(&&c[1]) {
                problems.push(format!(
                    "{name}::{} takes `&mut self`; allowed: {mutators:?}",
                    &c[1]
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "ADR-0002 D3:\n  {}",
        problems.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// D14: dogfooding

// @adr-0001-d14
#[test]
fn soqrates_checks_its_own_pull_requests_in_advisory_mode() {
    let workflow = read(workspace_root().join(".github/workflows/soqrates.yml"));
    for (needle, why) in [
        ("pull_request:", "runs on pull requests"),
        (
            "uses: ./",
            "runs the Soqrates action (which runs `soqrates ci`)",
        ),
        ("mode: advisory", "starts in advisory mode"),
        (
            "cancel-in-progress: false",
            "serialises runs so state updates never race",
        ),
    ] {
        assert!(
            workflow.contains(needle),
            "ADR-0001 D14: the workflow {why} (`{needle}`)"
        );
    }
    let action = read(workspace_root().join("action.yml"));
    assert!(
        action.contains("default: soqrates-state"),
        "ADR-0001 D14: learner state stays on the soqrates-state branch by default"
    );
    let project = read(workspace_root().join(".soqrates.toml"));
    assert!(
        project.contains(r#"mode = "advisory""#),
        "ADR-0001 D14: .soqrates.toml keeps dogfooding advisory"
    );
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
    /// `false` for a proposed ADR: its tags are valid, but its decisions
    /// need not be enforced yet.
    accepted: bool,
}

impl Decision {
    fn tag(&self) -> String {
        format!("adr-{}-{}", self.adr, self.id.to_lowercase())
    }
}

/// All decisions of accepted and proposed ADRs in docs/adr. Rejected and
/// superseded ADRs are ignored, so tags pointing at them become dangling.
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
        let accepted = if text.contains("**Status:** Accepted") {
            true
        } else if text.contains("**Status:** Proposed") {
            false
        } else {
            continue;
        };
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
                    accepted,
                });
            }
        }
        if let Some((id, _)) = current {
            panic!("ADR-{num} {id}: missing **Enforcement:** line");
        }
    }
    out
}

/// Tag -> places that reference it, from feature files and from test
/// comments anywhere in the workspace.
fn references() -> BTreeMap<String, Vec<String>> {
    let spec = Path::new(env!("CARGO_MANIFEST_DIR"));
    let spec_canonical = spec.canonicalize().unwrap();
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
    // Tagged unit and integration tests in the other crates count too, e.g.
    // the GitHub adapter's rendering tests (ADR-0004).
    for krate in fs::read_dir(workspace_root().join("crates")).unwrap() {
        let krate = krate.unwrap().path();
        if krate.canonicalize().unwrap() == spec_canonical {
            continue;
        }
        for dir in ["src", "tests"] {
            walk(&krate.join(dir), &mut |p| {
                if p.extension().is_some_and(|e| e == "rs") {
                    visit(p, true)
                }
            });
        }
    }
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
            _ if !d.accepted => format!("proposed: {places} test(s)"),
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
