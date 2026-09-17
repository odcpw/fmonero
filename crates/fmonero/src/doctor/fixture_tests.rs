//! The fixture-driven half of the doctor's tests (DESIGN.md §7.1, §8 Phase 0).
//!
//! §7.1: "Fixtures where parsing happens, a table test where it does not." A captured-input
//! fixture in `tests/fixtures/doctor/<check-id>[__<variant>]/{input/, expect.json}` is
//! required for the **seven** detectors that parse structured text — `node_layout`,
//! `node_data_dir_single_writer`, `ports_owned`, `units_present`,
//! `unit_no_password_in_argv`, `settings_valid`, `history_cache_fresh` — and this module is
//! what runs them. Phase 0 wrote the bytes; §8 hands the directory to this phase.
//!
//! **Each case runs the real [`Probes::capture_*`](super::probes::Probes) methods over the
//! fixture and then the real detector**, so the parsing is under test and not only the
//! predicate. Nothing here sets an environment variable: the process environment is global
//! and `cargo test` runs these in parallel threads, so every seam is applied to the
//! `Context` directly —
//!
//! | `expect.json` `_env` key | applied as |
//! |---|---|
//! | `FMONERO_PROC_ROOT` | `ctx.paths.proc_root` |
//! | `FMONERO_SYSTEMCTL_FIXTURE` | `ctx.systemctl = Systemctl::Fixture(..)` |
//! | `FMONERO_CACHE_DIR` | `ctx.paths.cache_dir` |
//! | `FMONERO_CONFIG_DIR` | `ctx.paths.config_dir` |
//! | `FMONERO_SYSTEMD_USER_DIR` | `ctx.paths.systemd_user_dir` |
//!
//! — which is the same set of overrides the binary honours, reached by the field each one
//! resolves to rather than through `std::env`. The last row is the gap Phase 0 reported and
//! `probes::systemd_user_dir` resolves; see `decisions/phase2-doctor.md`.

use std::path::{Path, PathBuf};

use fmonero_core::{settings::Settings, Context, Systemctl};

use super::checks::{CheckStatus, CHECKS};
use super::probes::Probes;
use super::DoctorGroup;

/// What a case's `expect.json` pins, minus the `_provenance` prose.
#[derive(Debug, serde::Deserialize)]
struct Expect {
    #[serde(default)]
    _env: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    _settings: std::collections::BTreeMap<String, String>,
    /// Probe facts the case asserts about the machine it describes, which no `input/` file
    /// can carry: whether the two binaries are installed, and the open wallet's height and
    /// primary address. Added by this phase, which owns `tests/fixtures/doctor/**` (§8).
    #[serde(default)]
    _probe: ProbeGiven,
    id: String,
    group: String,
    severity: String,
    class: String,
    status: String,
    fixable: bool,
    #[serde(default)]
    detail_contains: Vec<String>,
}

#[derive(Debug, Default, serde::Deserialize)]
struct ProbeGiven {
    #[serde(default)]
    monerod_installed: bool,
    #[serde(default)]
    wallet_rpc_installed: bool,
    #[serde(default)]
    wallet_height: Option<u64>,
    #[serde(default)]
    wallet_primary_address: Option<String>,
}

fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("the workspace root is two levels above crates/fmonero")
        .join("tests/fixtures/doctor")
}

/// Copy a tree, **preserving symlinks**: `ports_owned`'s `proc/<pid>/fd/<n>` entries are
/// dangling links to `socket:[<inode>]` on purpose, and the scan reads only the target.
fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create the copy root");
    for entry in std::fs::read_dir(from).expect("read the fixture tree") {
        let entry = entry.expect("dir entry");
        let src = entry.path();
        let dst = to.join(entry.file_name());
        let meta = std::fs::symlink_metadata(&src).expect("stat");
        if meta.file_type().is_symlink() {
            let target = std::fs::read_link(&src).expect("readlink");
            let _ = std::fs::remove_file(&dst);
            std::os::unix::fs::symlink(target, &dst).expect("symlink");
        } else if meta.is_dir() {
            copy_tree(&src, &dst);
        } else {
            std::fs::copy(&src, &dst).expect("copy");
        }
    }
}

/// Phase 0's recommendation (a) for `node_data_dir_single_writer__via_config_file`: the
/// fixture's `/proc/<pid>/cmdline` names `/etc/monerod.conf`, which no agent on this machine
/// can create (§1.1), so the copied cmdline is rewritten to point at the copied conf. One
/// NUL-joined argv, no product code.
fn point_config_file_at_the_copy(proc_root: &Path, conf: &Path) {
    let Ok(entries) = std::fs::read_dir(proc_root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path().join("cmdline");
        let Ok(raw) = std::fs::read(&path) else {
            continue;
        };
        let argv: Vec<Vec<u8>> = raw.split(|b| *b == 0).map(<[u8]>::to_vec).collect();
        if !argv.iter().any(|a| a == b"/etc/monerod.conf") {
            continue;
        }
        let mut out = Vec::new();
        for arg in argv {
            if arg == b"/etc/monerod.conf" {
                out.extend_from_slice(conf.as_os_str().as_encoded_bytes());
            } else {
                out.extend_from_slice(&arg);
            }
            out.push(0);
        }
        // `split` on a trailing NUL yields one empty tail element, which the loop above
        // re-emits as a bare NUL; the kernel's own format ends in exactly one.
        while out.ends_with(&[0, 0]) {
            out.pop();
        }
        std::fs::write(&path, out).expect("rewrite the cmdline fixture");
    }
}

/// Build the `Context` one case describes, and capture the probes its group needs.
fn probes_for(case: &Path, expect: &Expect, root: &Path) -> Probes {
    let input_copy = root.join("input");
    if case.join("input").is_dir() {
        copy_tree(&case.join("input"), &input_copy);
    } else {
        std::fs::create_dir_all(&input_copy).expect("create an empty input copy");
    }

    // `_env` values are paths relative to the case directory. Anything under `input/` is
    // taken from the copy (so it can be rewritten); a `../other-case/...` value is read
    // straight from the source tree, where it is another case's input.
    let resolve = |value: &str| -> PathBuf {
        match value.strip_prefix("input/") {
            Some(rest) => input_copy.join(rest),
            None if value == "input" => input_copy.clone(),
            None => case.join(value),
        }
    };

    let mut ctx = Context::for_test(root).expect("a test context");
    // An empty /proc unless the case supplies one, so a case is hermetic rather than
    // reading this machine's process table.
    ctx.paths.proc_root = root.join("empty-proc");
    std::fs::create_dir_all(&ctx.paths.proc_root).expect("create the empty proc root");
    // A refused loopback connect must not wait three seconds per case.
    ctx.timeouts.node = std::time::Duration::from_secs(1);
    ctx.timeouts.wallet = std::time::Duration::from_secs(1);
    ctx.timeouts.systemctl = std::time::Duration::from_secs(2);
    // Nothing in the fixture tree points at a real systemd user directory, and the default
    // is the OWNER's: point it at an empty directory so a case that does not supply units
    // sees none rather than this machine's.
    ctx.paths.systemd_user_dir = root.join("empty-systemd-user");
    std::fs::create_dir_all(&ctx.paths.systemd_user_dir).expect("create an empty unit dir");

    for (key, value) in &expect._env {
        let path = resolve(value);
        match key.as_str() {
            "FMONERO_PROC_ROOT" => ctx.paths.proc_root = path,
            "FMONERO_SYSTEMCTL_FIXTURE" => ctx.systemctl = Systemctl::Fixture(path),
            "FMONERO_CACHE_DIR" => ctx.paths.cache_dir = path,
            "FMONERO_CONFIG_DIR" => ctx.paths.config_dir = path,
            "FMONERO_SYSTEMD_USER_DIR" => ctx.paths.systemd_user_dir = path,
            other => panic!("{}: unknown _env key {other}", case.display()),
        }
    }

    // `_settings` is written into config.toml DIRECTLY rather than through
    // `Settings::set`, because `node_layout__remote` needs a non-loopback `node.url` and
    // `settings set node.url` refuses one outright (§2c). `Settings::load` does no value
    // validation, so it round-trips the document as written.
    if !expect._settings.is_empty() {
        let mut doc: toml::Value = toml::from_str(
            &String::from_utf8(Settings::default().to_toml_bytes().unwrap()).unwrap(),
        )
        .expect("the default settings document");
        for (key, value) in &expect._settings {
            let (section, leaf) = key.split_once('.').expect("a dotted settings key");
            let table = doc
                .get_mut(section)
                .and_then(toml::Value::as_table_mut)
                .unwrap_or_else(|| panic!("{key}: no [{section}] table"));
            let parsed: toml::Value = value
                .parse::<i64>()
                .map(toml::Value::Integer)
                .unwrap_or_else(|_| toml::Value::String(value.clone()));
            table.insert(leaf.to_string(), parsed);
        }
        std::fs::create_dir_all(&ctx.paths.config_dir).expect("config dir");
        std::fs::write(
            ctx.paths.config_file(),
            toml::to_string_pretty(&doc).expect("serialise"),
        )
        .expect("write config.toml");
    }
    // Deliberately lenient: `settings_valid__unparsable`'s whole point is a document that
    // does not load, and the detector reads the probe's text rather than `ctx.settings`.
    ctx.settings = Settings::load(&ctx.paths).unwrap_or_default();

    let def = CHECKS
        .iter()
        .find(|c| c.id == expect.id)
        .unwrap_or_else(|| panic!("{}: no check named {}", case.display(), expect.id));

    let mut p = Probes::empty(&ctx, Some(def.group));
    // The two facts a fixture cannot put in a file. A `node` case describes a machine where
    // monerod IS installed — otherwise every node detector correctly answers
    // `skip:monerod_not_installed` and the case would be testing the precondition.
    if expect._probe.monerod_installed {
        p.which_monerod = Some("/usr/bin/monerod".to_string());
    }
    if expect._probe.wallet_rpc_installed {
        p.which_wallet_rpc = Some("/usr/bin/monero-wallet-rpc".to_string());
    }
    p.wallet_height = expect._probe.wallet_height;
    p.wallet_primary_address = expect._probe.wallet_primary_address.clone();

    // The one rewrite Phase 0 recommended, applied to the copy.
    let conf = input_copy.join("monerod.conf");
    if conf.is_file() {
        point_config_file_at_the_copy(&ctx.paths.proc_root, &conf);
    }

    p.capture_units(&ctx);
    match def.group {
        DoctorGroup::Node => p.capture_node(&ctx),
        DoctorGroup::Wallet => p.capture_wallet_local(&ctx),
        DoctorGroup::Security => p.capture_security(&ctx, Some(DoctorGroup::Security)),
        DoctorGroup::Storage => p.capture_storage(&ctx),
        DoctorGroup::Install => p.capture_install(&ctx),
        DoctorGroup::Binaries | DoctorGroup::Units => {}
    }
    p
}

/// Run every case under `tests/fixtures/doctor/` and hold each to its `expect.json`.
#[test]
fn every_doctor_fixture_case_answers_as_its_expectation_says() {
    let root = fixtures_dir();
    let mut cases: Vec<PathBuf> = std::fs::read_dir(&root)
        .expect("tests/fixtures/doctor exists")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.join("expect.json").is_file())
        .collect();
    cases.sort();
    assert!(
        cases.len() >= 22,
        "Phase 0 wrote 22 cases; found {}",
        cases.len()
    );

    let work = std::env::temp_dir().join(format!("fmonero-doctor-fixtures-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&work);
    let mut covered: Vec<&str> = Vec::new();
    let mut failures: Vec<String> = Vec::new();

    for case in &cases {
        let name = case.file_name().unwrap().to_string_lossy().into_owned();
        let text = std::fs::read_to_string(case.join("expect.json")).expect("read expect.json");
        let expect: Expect = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{name}: expect.json does not parse: {e}"));
        let def = CHECKS
            .iter()
            .find(|c| c.id == expect.id)
            .unwrap_or_else(|| panic!("{name}: no check named {}", expect.id));
        covered.push(def.id);

        let case_root = work.join(&name);
        let p = probes_for(case, &expect, &case_root);
        let v = (def.detect)(&p);

        let mut bad = |what: String| failures.push(format!("{name}: {what}"));
        if def.group.name() != expect.group {
            bad(format!(
                "group is {} but the fixture says {}",
                def.group.name(),
                expect.group
            ));
        }
        if def.severity.name() != expect.severity {
            bad(format!(
                "severity is {} but the fixture says {}",
                def.severity.name(),
                expect.severity
            ));
        }
        if def.class.name() != expect.class {
            bad(format!(
                "class is {} but the fixture says {}",
                def.class.name(),
                expect.class
            ));
        }
        if v.status.name() != expect.status {
            bad(format!(
                "status is {}{} but the fixture says {} — detail: {}",
                v.status.name(),
                v.reason.map(|r| format!(":{r}")).unwrap_or_default(),
                expect.status,
                v.detail
            ));
        }
        if v.plan.is_some() != expect.fixable {
            bad(format!(
                "fixable is {} but the fixture says {}",
                v.plan.is_some(),
                expect.fixable
            ));
        }
        for needle in &expect.detail_contains {
            if !v.detail.contains(needle) {
                bad(format!(
                    "the detail does not contain '{needle}': {}",
                    v.detail
                ));
            }
        }
    }

    // §7.1 names seven detectors that must be fixture-driven. A fixture directory that
    // stopped covering one of them is a coverage hole this assertion closes.
    for id in [
        "node_layout",
        "node_data_dir_single_writer",
        "ports_owned",
        "units_present",
        "unit_no_password_in_argv",
        "settings_valid",
        "history_cache_fresh",
    ] {
        assert!(
            covered.contains(&id),
            "no fixture case drives the §7.1 parsing detector {id}"
        );
    }

    let _ = std::fs::remove_dir_all(&work);
    assert!(
        failures.is_empty(),
        "{} fixture case(s) disagreed:\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

/// The class-A round trip §8 asks for, in process: corrupt → `--fix` → assert → `undo
/// latest` → byte-compare, under a test state directory.
#[test]
fn a_class_a_fixer_round_trips_and_undo_gives_the_corrupt_bytes_back() {
    use super::checks::Class;
    use super::mutate::{self, Action, Mutator, RunDir};

    let dir = std::env::temp_dir().join(format!("fmonero-doctor-classa-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = Context::for_test(&dir).expect("test context");

    // 1. Corrupt: a state.json that does not parse, and a config.toml with one bad value.
    let corrupt_state = b"{ this is not json\n";
    let corrupt_settings = "[wallet]\npriority = 9\n";
    std::fs::write(ctx.paths.state_file(), corrupt_state).unwrap();
    std::fs::write(ctx.paths.config_file(), corrupt_settings).unwrap();

    let p = {
        let mut p = Probes::empty(&ctx, Some(DoctorGroup::Install));
        p.capture_install(&ctx);
        p
    };
    // 2. Both detectors see it.
    let state_def = CHECKS.iter().find(|c| c.id == "state_valid").unwrap();
    let settings_def = CHECKS.iter().find(|c| c.id == "settings_valid").unwrap();
    let sv = (state_def.detect)(&p);
    assert_eq!(sv.status, CheckStatus::Warn, "{}", sv.detail);
    assert_eq!(sv.plan, Some(super::checks::PlanKind::ResetStateJson));
    let cv = (settings_def.detect)(&p);
    assert_eq!(cv.status, CheckStatus::Warn, "{}", cv.detail);
    assert!(cv.detail.contains("wallet.priority"), "{}", cv.detail);

    // 3. --fix, through the one chokepoint.
    let run = RunDir::create(&ctx).expect("run dir");
    let run_dir = run.dir.clone();
    let mut m = Mutator::new(&ctx, Some(run), false);
    for (def, plan) in [
        (state_def, super::checks::PlanKind::ResetStateJson),
        (settings_def, super::checks::PlanKind::RepairSettings),
    ] {
        for action in mutate::plan(plan, &ctx, &p).expect("plan") {
            let is_write = matches!(action, Action::WriteFile { .. });
            assert!(is_write, "a class-A fixer writes a file and nothing else");
            m.mutate(def.id, Class::A, action).expect("apply");
        }
    }
    drop(m);

    // 4. Assert: both files are now valid, and the repaired settings kept nothing bad.
    let after = {
        let mut p = Probes::empty(&ctx, Some(DoctorGroup::Install));
        p.capture_install(&ctx);
        p
    };
    assert_eq!((state_def.detect)(&after).status, CheckStatus::Pass);
    assert_eq!(
        (settings_def.detect)(&after).status,
        CheckStatus::Pass,
        "{}",
        (settings_def.detect)(&after).detail
    );
    assert_eq!(
        Settings::load(&ctx.paths)
            .expect("repaired settings load")
            .wallet
            .priority,
        Settings::default().wallet.priority
    );

    // 5. undo latest → the corrupt bytes, byte for byte. That is the whole point: a fixer
    // whose undo cannot give the original back is a fixer nobody dares run.
    let latest = std::fs::canonicalize(ctx.paths.doctor_latest()).expect("latest resolves");
    assert_eq!(latest, std::fs::canonicalize(&run_dir).unwrap());
    let steps = mutate::undo(&ctx, &latest).expect("undo");
    assert!(steps.iter().all(|s| s.ok), "{steps:?}");
    assert_eq!(
        std::fs::read(ctx.paths.state_file()).unwrap(),
        corrupt_state
    );
    assert_eq!(
        std::fs::read_to_string(ctx.paths.config_file()).unwrap(),
        corrupt_settings
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// §7's read-only default, as a measurement rather than a claim: a run with neither `--fix`
/// nor `--save` leaves the state directory byte-identical.
#[test]
fn a_detect_only_run_writes_nothing_under_the_state_directory() {
    let dir = std::env::temp_dir().join(format!("fmonero-doctor-readonly-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let ctx = Context::for_test(&dir).expect("test context");
    let before = checksum_tree(&ctx.paths.state_dir);

    let p = Probes::capture(&ctx, None);
    for def in CHECKS {
        let _ = (def.detect)(&p);
    }
    assert_eq!(
        before,
        checksum_tree(&ctx.paths.state_dir),
        "capturing probes and running every detector must change nothing"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every path and its bytes under `root`, sorted — a cheap state-directory checksum.
fn checksum_tree(root: &Path) -> Vec<(String, u64, Vec<u8>)> {
    let mut out = Vec::new();
    fn walk(path: &Path, out: &mut Vec<(String, u64, Vec<u8>)>) {
        let Ok(entries) = std::fs::read_dir(path) else {
            return;
        };
        for entry in entries.flatten() {
            let p = entry.path();
            let Ok(meta) = std::fs::symlink_metadata(&p) else {
                continue;
            };
            if meta.is_dir() {
                out.push((p.display().to_string(), 0, Vec::new()));
                walk(&p, out);
            } else {
                out.push((
                    p.display().to_string(),
                    meta.len(),
                    std::fs::read(&p).unwrap_or_default(),
                ));
            }
        }
    }
    walk(root, &mut out);
    out.sort();
    out
}
