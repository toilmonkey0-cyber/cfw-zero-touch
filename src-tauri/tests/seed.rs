use std::fs;

use cfw_zero_touch_lib::seed::seed_folders;
use cfw_zero_touch_lib::volume::VolumeDecision;

fn scratch() -> std::path::PathBuf {
    // Unique even when parallel tests scratch a directory within one clock
    // tick. They share the pid, and Windows often reports the same
    // subsec_nanos for that whole tick. One test's remove_dir_all then
    // deletes another test's folders.
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cfw-seed-{}-{}-{seq}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn parallel_scratch_directories_are_distinct() {
    let paths: Vec<_> = std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for _ in 0..8 {
            handles.push(scope.spawn(scratch));
        }
        handles
            .into_iter()
            .map(|handle| handle.join().expect("scratch thread"))
            .collect()
    });
    let mut unique = paths.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), paths.len());
    for path in paths {
        let _ = fs::remove_dir_all(path);
    }
}

#[test]
fn seed_creates_the_folder_map_and_records_it() {
    let root = scratch();
    let created = seed_folders(
        &root,
        &["gba".into(), "snes".into(), "bios".into()],
        &VolumeDecision::Ready { relabel: false },
    )
    .unwrap();

    assert_eq!(created, vec!["gba", "snes", "bios"]);
    assert!(root.join("gba").is_dir());
    assert!(root.join("bios").is_dir());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn seed_refuses_a_card_that_still_needs_a_format() {
    let root = scratch();
    let error = seed_folders(
        &root,
        &["gba".into()],
        &VolumeDecision::NeedsFormat {
            reason: "volume is not empty".into(),
        },
    )
    .unwrap_err();

    assert!(error.to_lowercase().contains("not empty") || error.to_lowercase().contains("format"));
    assert!(!root.join("gba").exists());
    let _ = fs::remove_dir_all(&root);
}

#[test]
fn seed_rejects_paths_that_escape_the_card() {
    let root = scratch();
    let folders = vec![format!("../cfw-seed-escape-{}", std::process::id())];
    let error =
        seed_folders(&root, &folders, &VolumeDecision::Ready { relabel: false }).unwrap_err();

    assert!(error.contains("..") || error.to_lowercase().contains("escape"));
    assert!(!root
        .join("..")
        .join(format!("cfw-seed-escape-{}", std::process::id()))
        .exists());
    let _ = fs::remove_dir_all(&root);
}
