use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{Duration, SystemTime};

fn fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixture")
}

fn solff(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_solff"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("run solff")
}

fn stdout(o: &Output) -> String {
    String::from_utf8(o.stdout.clone()).unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8(o.stderr.clone()).unwrap()
}

/// Fixture query without touching the fixture directory (no cache).
fn query(args: &[&str]) -> Output {
    let mut a = vec!["--no-cache"];
    a.extend_from_slice(args);
    solff(&fixture(), &a)
}

struct TempProject(PathBuf);

impl TempProject {
    /// Copy of the fixture's sources and artifacts, in its own directory.
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("solff-test-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        for sub in ["src", "test", "out"] {
            copy_dir(&fixture().join(sub), &dir.join(sub));
        }
        TempProject(dir)
    }
}

impl Drop for TempProject {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn bump_mtime(path: &Path) {
    let f = fs::OpenOptions::new().write(true).open(path).unwrap();
    f.set_modified(SystemTime::now() + Duration::from_secs(60))
        .unwrap();
}

#[test]
fn override_creates_inherited_entry_points() {
    let out = stdout(&query(&["record"]));
    // Inherited external function is an entry point on each concrete child,
    // reached through the hook override.
    assert!(
        out.contains("  Vault._hook\n    Base.entry <- Vault\n"),
        "{out}"
    );
    assert!(
        out.contains("  Other._hook\n    Base.entry <- Other\n"),
        "{out}"
    );
    // Quiet does not override the hook, so Base.entry does not reach the target there.
    assert!(!out.contains("Base.entry <- Quiet"), "{out}");
}

#[test]
fn abstract_contracts_and_excluded_files_are_not_shown() {
    let out = stdout(&query(&["record"]));
    // Base is abstract: its code shows up, but an entry point is always on a concrete contract.
    for line in out
        .lines()
        .filter(|l| l.trim_start().starts_with("Base.") && l.contains("<-"))
    {
        assert!(
            !line.ends_with("<-") && !line.contains("<- Base"),
            "{line}\n{out}"
        );
    }
    assert!(!out.contains("FixtureTest"), "{out}");
}

#[test]
fn tree_is_rooted_at_the_function_of_interest() {
    let out = stdout(&query(&["record"]));
    let tops: Vec<&str> = out.lines().filter(|l| !l.starts_with(' ')).collect();
    assert_eq!(tops, ["Sink.record <-"], "{out}");
}

#[test]
fn shared_paths_are_not_duplicated() {
    let out = stdout(&query(&["record"]));
    assert!(!out.contains("see above"), "{out}");
    // The part between the function of interest and the fork prints once.
    assert_eq!(out.matches("Vault._shared").count(), 1, "{out}");
    assert!(
        out.contains("  Vault._shared\n    Vault._viaA\n      Vault.diamond <-\n    Vault._viaB\n      Vault.diamond <-\n"),
        "{out}"
    );
}

#[test]
fn entry_point_contract_follows_the_arrow() {
    let out = stdout(&query(&["record"]));
    // Inherited chain: every line names where the code lives; only the entry point names its contract.
    assert!(
        out.contains("  Base._inner\n    Base._mid\n      Base.chain <- Vault\n"),
        "{out}"
    );
    // Declaring == concrete: nothing to add after the arrow.
    assert!(out.contains("Vault.diamond <-\n"), "{out}");
    assert!(!out.contains('@') && !out.contains("(from"), "{out}");
}

#[test]
fn constructor_reached_through_new() {
    let out = stdout(&query(&["record"]));
    assert!(
        out.contains("  Vault._ctorPing\n    Vault.constructor <-\n      Router.make <-\n"),
        "{out}"
    );
}

#[test]
fn hard_mode_has_no_guessed_calls() {
    let out = stdout(&query(&["record"]));
    assert!(!out.contains('~'), "{out}");
    assert!(!out.contains("Router.route"), "{out}");
}

#[test]
fn soft_mode_shows_only_chains_with_a_guessed_call() {
    let out = stdout(&query(&["--soft", "record"]));
    assert_eq!(
        out,
        "Sink.record <-\n  Base.tracked [modifier]\n    Vault.deposit <-\n      ~Router.route <-\n"
    );
    assert_eq!(
        stdout(&query(&["--soft", "deposit"])),
        "Vault.deposit <-\n  ~Router.route <-\n"
    );
    let o = query(&["--soft", "rec"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("no entry point reaches `rec` through a `~` call"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn library_free_function_super_and_this() {
    let out = stdout(&query(&["record"]));
    assert!(
        out.contains("  Lib.sol.freeHelper\n    Vault.viaFree <-\n"),
        "{out}"
    );
    assert!(
        out.contains("  RecLib.viaLib\n    Vault.viaLib <-\n"),
        "{out}"
    );
    assert!(
        out.contains("  Base._baseSuper\n    Vault._baseSuper\n      Vault.callSuper <-\n"),
        "{out}"
    );
    assert!(
        out.contains("  Base.tracked [modifier]\n    Vault.deposit <-\n      Vault.viaThis <-\n"),
        "{out}"
    );
}

#[test]
fn recursion_is_marked() {
    let out = stdout(&query(&["Vault.rec"]));
    assert_eq!(out, "Base.rec <- Vault\n  Base.rec <- Vault (recursive)\n");
}

#[test]
fn a_target_calling_another_target_is_nested() {
    // Both Vault.viaLib and RecLib.viaLib match; the first calls the second.
    assert_eq!(
        stdout(&query(&["viaLib"])),
        "RecLib.viaLib\n  Vault.viaLib <-\n"
    );
}

#[test]
fn target_filters() {
    let out = stdout(&query(&["deposit"]));
    assert_eq!(out, "Vault.deposit <-\n  Vault.viaThis <-\n");
    // By selector: same nodes.
    assert_eq!(stdout(&query(&["0xb6b55f25"])), out);
    // By declaring and concrete contract.
    assert_eq!(stdout(&query(&["Vault.deposit"])), out);
    // Overloads are told apart by signature.
    let o = stdout(&query(&["overloaded(uint256)"]));
    assert!(
        o.contains("Base.overloaded(uint256) <- Other, Quiet, Vault"),
        "{o}"
    );
    assert!(!o.contains("address"), "{o}");
}

#[test]
fn instances_of_one_function_share_a_top_line() {
    // Base.overloaded is inherited by several contracts: one line, not one per contract.
    let o = stdout(&query(&["overloaded(uint256)"]));
    assert_eq!(o.matches("Base.overloaded(uint256)").count(), 1, "{o}");
}

#[test]
fn multiple_names_get_headers() {
    let out = stdout(&query(&["deposit", "Vault.rec"]));
    assert!(out.starts_with("# deposit\n"), "{out}");
    assert!(out.contains("\n\n# Vault.rec\n"), "{out}");
}

#[test]
fn clean_run_prints_nothing_on_stderr() {
    // The fixture has an unresolvable `.call`; it is not reported.
    let o = query(&["deposit"]);
    assert_eq!(stderr(&o), "");
}

#[test]
fn unknown_function_fails() {
    let o = query(&["doesNotExist"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no function matching `doesNotExist`"));
}

#[test]
fn missing_asts_fail() {
    let p = TempProject::new("noast");
    for entry in fs::read_dir(p.0.join("out")).unwrap() {
        let dir = entry.unwrap().path();
        if dir.is_dir() && dir.file_name().unwrap() != "build-info" {
            for f in fs::read_dir(&dir).unwrap() {
                fs::write(f.unwrap().path(), r#"{"abi":[]}"#).unwrap();
            }
        }
    }
    let o = solff(&p.0, &["--no-cache", "record"]);
    assert!(!o.status.success());
    assert!(
        stderr(&o).contains("no ASTs found under out"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn stale_artifacts_for_deleted_sources_are_skipped() {
    let p = TempProject::new("stale");
    fs::remove_file(p.0.join("src/Router.sol")).unwrap();
    let o = solff(&p.0, &["--no-cache", "record"]);
    assert!(o.status.success());
    assert!(!stdout(&o).contains("Router"), "{}", stdout(&o));
    assert!(!stderr(&o).contains("skipped"), "{}", stderr(&o));
}

#[test]
fn cache_matches_uncached_and_invalidates() {
    let p = TempProject::new("cache");
    let cache = p.0.join(".solff/graph.bin");

    let uncached = solff(&p.0, &["--no-cache", "record"]);
    assert!(!cache.exists());

    let cold = solff(&p.0, &["record"]);
    assert!(cache.exists());
    assert_eq!(
        fs::read_to_string(p.0.join(".solff/.gitignore")).unwrap(),
        "*\n"
    );
    let warm = solff(&p.0, &["record"]);
    for o in [&cold, &warm] {
        assert_eq!(stdout(o), stdout(&uncached));
        assert_eq!(stderr(o), stderr(&uncached));
    }

    // Touching an artifact changes the fingerprint and rewrites the cache.
    let before = fs::read(&cache).unwrap();
    bump_mtime(&p.0.join("out/Sink.sol/Sink.json"));
    solff(&p.0, &["record"]);
    let after_touch = fs::read(&cache).unwrap();
    assert_ne!(before, after_touch);

    // Removing an artifact does too, and the answer follows.
    // (Other.sol holds two contracts, both artifacts carry its AST.)
    fs::remove_file(p.0.join("out/Other.sol/Other.json")).unwrap();
    fs::remove_file(p.0.join("out/Other.sol/Quiet.json")).unwrap();
    let o = solff(&p.0, &["record"]);
    assert_ne!(fs::read(&cache).unwrap(), after_touch);
    assert!(stdout(&o).contains("Base.entry <- Vault"), "{}", stdout(&o));
    assert!(!stdout(&o).contains("<- Other"), "{}", stdout(&o));

    // A corrupt cache is a miss, not an error.
    fs::write(&cache, b"garbage").unwrap();
    let o = solff(&p.0, &["record"]);
    assert!(o.status.success());
    assert_ne!(fs::read(&cache).unwrap(), b"garbage");
}

#[test]
fn deleted_source_invalidates_cache() {
    let p = TempProject::new("srcdel");
    solff(&p.0, &["record"]);
    fs::remove_file(p.0.join("src/Router.sol")).unwrap();
    let o = solff(&p.0, &["record"]);
    assert!(!stdout(&o).contains("Router"), "{}", stdout(&o));
}

#[test]
fn mixed_compilation_jobs_are_flagged() {
    let o = query(&["record"]);
    assert!(!stderr(&o).contains("compilation jobs"), "{}", stderr(&o));

    // Point every reference in Router at a declaration that exists nowhere,
    // as happens when only some artifacts were rebuilt.
    let p = TempProject::new("mixed");
    let path = p.0.join("out/Router.sol/Router.json");
    let text = fs::read_to_string(&path).unwrap();
    let key = "\"referencedDeclaration\":";
    let mut out = String::new();
    let mut rest = text.as_str();
    while let Some(i) = rest.find(key) {
        let (head, tail) = rest.split_at(i + key.len());
        out.push_str(head);
        out.push_str("99999999");
        rest = tail.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    }
    out.push_str(rest);
    fs::write(&path, out).unwrap();
    let o = solff(&p.0, &["--no-cache", "record"]);
    assert!(
        stderr(&o).contains("artifacts likely come from different compilation jobs"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn list_contract_functions() {
    let o = query(&["--list", "Vault"]);
    assert!(o.status.success());
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"deposit"));
    assert!(lines.contains(&"entry (from Base)"));
    // An override appears once, as the most derived.
    assert_eq!(lines.iter().filter(|l| l.starts_with("_hook")).count(), 1);
    assert!(lines.contains(&"_hook"));

    let o = query(&["--list", "Nope"]);
    assert!(!o.status.success());
    assert!(stderr(&o).contains("no contract matching `Nope`"));
}

#[test]
fn entries_lists_only_entry_points() {
    let o = query(&["--entries", "record"]);
    assert!(o.status.success());
    let out = stdout(&o);
    let lines: Vec<&str> = out.lines().collect();
    assert!(lines.contains(&"Vault.diamond"));
    assert!(lines.contains(&"Vault.entry"));
    assert!(lines.contains(&"Other.entry"));
    assert!(lines.contains(&"Vault.constructor"));
    assert!(!lines.contains(&"Quiet.entry"));
    assert!(!lines.contains(&"Router.route"));
    // Nothing from in between, no tree markup, one per line, sorted, no repeats.
    assert!(
        !lines.contains(&"Vault._shared") && !out.contains("<-") && !out.contains("  "),
        "{out}"
    );
    let mut sorted = lines.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(lines, sorted);

    // Soft mode: only entry points whose chain has a guessed call.
    let out = stdout(&query(&["--entries", "--soft", "record"]));
    assert!(out.lines().any(|l| l == "Router.route"), "{out}");
    assert!(!out.lines().any(|l| l == "Vault.diamond"), "{out}");
}
