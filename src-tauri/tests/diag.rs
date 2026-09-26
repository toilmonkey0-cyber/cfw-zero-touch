use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

use cfw_zero_touch_lib::diag::{log, log_path, timestamp_from_unix};

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
    assert!(lines[1].ends_with(" WARN event_two detail two"), "{lines:?}");
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
