use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use std::path::Path;

use cfw_zero_touch_lib::diag::{
    build_support_zip, format_fake_card, format_identity, format_probe, log, log_path,
    read_stored_zip, refuse_support_destination, remember_fake_card, remember_identity,
    remember_probe, timestamp_from_unix, write_support_bundle, SupportFacts,
};

// Both env-dependent tests share this binary; serialize their
// CFW_STUDIO_DATA manipulation so parallel threads cannot flake.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn scratch_data(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "cfw-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    std::env::set_var("CFW_STUDIO_DATA", &path);
    path
}

#[test]
fn timestamps_are_utc_and_handle_leap_days() {
    assert_eq!(timestamp_from_unix(0), "1970-01-01T00:00:00Z");
    assert_eq!(timestamp_from_unix(951782400), "2000-02-29T00:00:00Z");
    assert_eq!(timestamp_from_unix(1000000000), "2001-09-09T01:46:40Z");
    assert_eq!(timestamp_from_unix(86399), "1970-01-01T23:59:59Z");
    assert_eq!(timestamp_from_unix(86400), "1970-01-02T00:00:00Z");
}

#[test]
fn log_appends_timestamped_lines_under_the_store_root() {
    let _env = ENV_LOCK.lock().unwrap();
    let root = scratch_data("diag-append");
    log("INFO", "event_one", "detail one");
    log("WARN", "event_two", "detail two");

    let text = fs::read_to_string(log_path()).unwrap();
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 2);
    assert!(
        lines[0].starts_with("20") && lines[0].ends_with(" INFO event_one detail one"),
        "{lines:?}"
    );
    assert!(
        lines[1].ends_with(" WARN event_two detail two"),
        "{lines:?}"
    );
    assert!(log_path().starts_with(&root), "{:?}", log_path());
    let _ = fs::remove_dir_all(&root);
    std::env::remove_var("CFW_STUDIO_DATA");
}

#[test]
fn oversized_logs_rotate_to_old() {
    let _env = ENV_LOCK.lock().unwrap();
    let root = scratch_data("diag-rotate");
    {
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(log_path())
            .unwrap();
        // One entry over the 1 MiB rotation mark.
        for _ in 0..11 {
            writeln!(file, "{}", "x".repeat(100_000)).unwrap();
        }
    }

    log("INFO", "after_rotation", "fresh");

    assert!(
        log_path().with_extension("old").is_file(),
        "the oversized log must move to diagnostics.old"
    );
    let text = fs::read_to_string(log_path()).unwrap();
    assert_eq!(text.lines().count(), 1);
    assert!(text.ends_with(" INFO after_rotation fresh\n"));
    let _ = fs::remove_dir_all(&root);
    std::env::remove_var("CFW_STUDIO_DATA");
}

const SUPPORT_FILES: &[&str] = &[
    "README.txt",
    "version.txt",
    "os.txt",
    "logs/diagnostics.log",
    "logs/diagnostics.old",
    "probe.txt",
    "identity.txt",
    "fake-card.txt",
    "license.txt",
];

fn support_facts(library: Option<PathBuf>) -> SupportFacts {
    SupportFacts {
        version: "9.9.9-test".into(),
        os_name: "test-os".into(),
        arch: "test-arch".into(),
        library,
        license_state: "inactive".into(),
        license_reason: "not_activated".into(),
        license_message: "No license on this computer.".into(),
        license_uses: None,
        over_soft_limit: false,
        stale: false,
        signed_updates: false,
        support: false,
        safety_open: true,
        usernames: vec!["romcollector".into()],
        secrets: Vec::new(),
    }
}

fn entry<'a>(files: &'a [(String, Vec<u8>)], name: &str) -> &'a str {
    let (_, data) = files
        .iter()
        .find(|(got, _)| got == name)
        .unwrap_or_else(|| panic!("missing {name}"));
    std::str::from_utf8(data).unwrap_or_else(|_| panic!("{name} is not plain text"))
}

fn assert_absent(zip: &[u8], files: &[(String, Vec<u8>)], secret: &str) {
    let needle = secret.to_ascii_lowercase();
    assert!(
        !String::from_utf8_lossy(zip)
            .to_ascii_lowercase()
            .contains(&needle),
        "zip bytes still contain {secret}"
    );
    for (name, data) in files {
        assert!(
            !String::from_utf8_lossy(data)
                .to_ascii_lowercase()
                .contains(&needle),
            "{name} still contains {secret}"
        );
    }
}

#[test]
fn support_zip_hides_fixture_rom_names_and_the_library_path() {
    let _env = ENV_LOCK.lock().unwrap();
    let root = scratch_data("support-roms");
    let library = root.join("library");
    fs::create_dir_all(library.join("snes")).unwrap();
    let roms = [
        "Chrono Trigger (USA).sfc",
        "Pokemon Blue.gb",
        "Final Fantasy VI.smc",
        "Mother 3.gba",
    ];
    for name in roms {
        fs::write(library.join("snes").join(name), b"fixture").unwrap();
    }
    let library_text = library.display().to_string();
    let mut body = format!(
        "INFO copy_done library={library_text} file={} account=romcollector user=/home/romcollector/Documents\\Games C:\\Users\\romcollector\\Roms\\{}\n",
        roms[0], roms[1]
    );
    for name in roms {
        body.push_str(&format!("INFO kept {name}\n"));
    }
    fs::write(log_path(), body).unwrap();

    let zip = build_support_zip(&support_facts(Some(library)));
    let files = read_stored_zip(&zip).expect("zip opens");
    for secret in [
        "Chrono Trigger",
        "Pokemon Blue",
        "Final Fantasy VI",
        "Mother 3",
        "Chrono Trigger (USA).sfc",
        "Pokemon Blue.gb",
        "Final Fantasy VI.smc",
        "Mother 3.gba",
        library_text.as_str(),
        "romcollector",
    ] {
        assert_absent(&zip, &files, secret);
    }
    assert_absent(&zip, &files, &library_text.replace('\\', "/"));
    let notes = entry(&files, "logs/diagnostics.log");
    assert!(notes.contains("[rom]"), "{notes}");
    assert!(notes.contains("[path]"), "{notes}");
    assert!(notes.contains("[user]"), "{notes}");
    let _ = fs::remove_dir_all(&root);
    std::env::remove_var("CFW_STUDIO_DATA");
}

#[test]
fn support_zip_hides_the_license_key_email_and_gumroad_body() {
    let _env = ENV_LOCK.lock().unwrap();
    let root = scratch_data("support-license");
    let key = "ABCD-EFGH-IJKL-MN99";
    let echo = "ECHO-KEY-SHOULD-NOT-LEAK-9999";
    let email = "buyer@example.invalid";
    let mac = "ab".repeat(32);
    let token = "cached-license-token-DO-NOT-SHIP";
    let gumroad = include_str!("fixtures/gumroad/valid.json");
    assert!(gumroad.contains(echo));
    assert!(gumroad.contains(email));

    fs::create_dir_all(root.join("license")).unwrap();
    fs::write(
        root.join("license").join("activation.json"),
        format!(
            r#"{{"payload":{{"license_key":"{key}","email":"{email}"}},"mac":"{mac}","token":"{token}"}}"#
        ),
    )
    .unwrap();
    fs::write(root.join("gumroad-response.json"), gumroad).unwrap();
    fs::write(
        log_path(),
        format!(
            "INFO pasted {gumroad}\nINFO plain key={key} email={email} mac={mac} token={token}\n"
        ),
    )
    .unwrap();

    let mut facts = support_facts(None);
    facts.license_message = format!("leaked {key} and {email}");
    let zip = build_support_zip(&facts);
    let files = read_stored_zip(&zip).expect("zip opens");
    for secret in [
        key,
        echo,
        email,
        mac.as_str(),
        token,
        "buyer@",
        "fixture-seller",
        "signed build",
    ] {
        assert_absent(&zip, &files, secret);
    }
    assert!(files
        .iter()
        .all(|(name, _)| { !name.contains("activation") && !name.contains("gumroad") }));
    let license = entry(&files, "license.txt");
    assert!(license.contains("state=inactive"), "{license}");
    assert!(
        license.contains("[redacted]") || license.contains("[email]"),
        "{license}"
    );
    assert!(!license.contains(key));
    assert!(!license.contains(email));
    let _ = fs::remove_dir_all(&root);
    std::env::remove_var("CFW_STUDIO_DATA");
}

#[test]
fn support_zip_opens_with_readme_and_the_saved_card_results() {
    let _env = ENV_LOCK.lock().unwrap();
    let root = scratch_data("support-open");
    fs::write(log_path(), "INFO app_start version=9.9.9-test\n").unwrap();
    fs::write(log_path().with_extension("old"), "INFO older note\n").unwrap();
    let probe = format_probe(
        "finished",
        2,
        64_000_000_000,
        8_000_000_000,
        (12.5, 9.25),
        true,
        "This card is fake.",
    );
    let identity = format_identity(2, "fixture-reader", 64_000_000_000, "MBR-AABBCCDD");
    let fake = format_fake_card("fake", 64_000_000_000, 8_000_000_000, "This card is fake.");
    remember_probe(&probe);
    remember_identity(&identity);
    remember_fake_card(&fake);

    let before = walk_files(&root);
    let zip = build_support_zip(&support_facts(None));
    let after = walk_files(&root);
    assert_eq!(before, after, "building the zip must not write files");

    let files = read_stored_zip(&zip).expect("zip opens");
    let names: Vec<&str> = files.iter().map(|(name, _)| name.as_str()).collect();
    assert_eq!(names, SUPPORT_FILES);
    let readme = entry(&files, "README.txt");
    assert!(readme.starts_with("CFW Card Studio support file"));
    assert!(!readme.contains('\0'));
    assert!(readme
        .chars()
        .all(|ch| ch == '\n' || ch == '\t' || !ch.is_control()));
    for name in SUPPORT_FILES {
        if *name != "README.txt" {
            assert!(readme.contains(name), "readme should name {name}: {readme}");
        }
    }
    assert!(readme.to_lowercase().contains("license key"));
    assert_eq!(entry(&files, "version.txt").trim(), "version=9.9.9-test");
    let os = entry(&files, "os.txt");
    assert!(os.contains("os=test-os"), "{os}");
    assert!(os.contains("arch=test-arch"), "{os}");
    assert!(entry(&files, "logs/diagnostics.log").contains("app_start"));
    assert!(entry(&files, "logs/diagnostics.old").contains("older note"));
    assert!(entry(&files, "probe.txt").contains("status=finished"));
    assert!(entry(&files, "probe.txt").contains("This card is fake."));
    assert!(entry(&files, "identity.txt").contains("partition_table_id=MBR-AABBCCDD"));
    assert!(entry(&files, "identity.txt").contains("fixture-reader"));
    assert!(entry(&files, "fake-card.txt").contains("kind=fake"));
    let license = entry(&files, "license.txt");
    assert!(license.contains("state=inactive"));
    assert!(license.contains("reason=not_activated"));
    assert!(license.contains("safety_open=true"));
    assert_python_opens(&zip, SUPPORT_FILES);
    let _ = fs::remove_dir_all(&root);
    std::env::remove_var("CFW_STUDIO_DATA");
}

#[test]
fn support_file_refuses_a_disk_device_path() {
    let disk = refuse_support_destination(Path::new("/dev/mmcblk0")).unwrap_err();
    assert!(
        disk.to_lowercase().contains("disk") || disk.to_lowercase().contains("file"),
        "{disk}"
    );
    let drive = refuse_support_destination(Path::new(r"\\.\PhysicalDrive1")).unwrap_err();
    assert!(
        drive.to_lowercase().contains("disk") || drive.to_lowercase().contains("file"),
        "{drive}"
    );
    assert!(refuse_support_destination(Path::new("cfw-card-studio-support.zip")).is_ok());
    let written = write_support_bundle(Path::new("/dev/sda"), &support_facts(None)).unwrap_err();
    assert!(
        written.to_lowercase().contains("disk") || written.to_lowercase().contains("file"),
        "{written}"
    );
}

fn walk_files(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path.strip_prefix(root).unwrap().display().to_string());
            }
        }
    }
    found.sort();
    found
}

fn assert_python_opens(bytes: &[u8], names: &[&str]) {
    let python = ["python3", "python"].into_iter().find(|bin| {
        std::process::Command::new(bin)
            .arg("--version")
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    });
    let Some(python) = python else {
        return;
    };
    let dir = std::env::temp_dir().join(format!(
        "cfw-zip-open-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("support.zip");
    fs::write(&path, bytes).unwrap();
    let script = r#"
import sys, zipfile
path, expected = sys.argv[1], sys.argv[2].split(",")
with zipfile.ZipFile(path) as zf:
    bad = zf.testzip()
    if bad is not None:
        raise SystemExit(f"bad crc {bad}")
    found = zf.namelist()
    if found != expected:
        raise SystemExit(f"names {found!r} != {expected!r}")
    readme = zf.read("README.txt")
    if b"\0" in readme:
        raise SystemExit("readme has a nul")
print("ok")
"#;
    let output = std::process::Command::new(python)
        .arg("-c")
        .arg(script)
        .arg(&path)
        .arg(names.join(","))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "python could not open the zip: {}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let _ = fs::remove_dir_all(&dir);
}
