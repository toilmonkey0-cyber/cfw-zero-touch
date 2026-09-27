use std::fs;
use std::path::{Path, PathBuf};

use cfw_zero_touch_lib::profiles::SystemFolder;
use cfw_zero_touch_lib::romcopy::{execute_copy, plan_copy, CopyAction};

fn scratch(name: &str) -> PathBuf {
    // Unique even when two parallel test threads scratch the same name
    // within one nanosecond (they share the pid).
    static SEQ: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "cfw-{name}-{}-{}-{seq}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    ));
    fs::create_dir_all(&path).unwrap();
    path
}

fn systems() -> Vec<SystemFolder> {
    vec![
        SystemFolder {
            id: "gba".into(),
            folder: "gba".into(),
            extensions: vec![".gba".into(), ".zip".into()],
            dat_name_pattern: None,
        },
        SystemFolder {
            id: "snes".into(),
            folder: "snes".into(),
            extensions: vec![".sfc".into()],
            dat_name_pattern: None,
        },
    ]
}

#[test]
fn plan_keeps_matching_extensions_and_drops_the_rest() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::write(library.join("gba").join("game.gba"), b"rom").unwrap();
    fs::write(library.join("gba").join("notes.txt"), b"nope").unwrap();
    fs::create_dir_all(library.join("bios")).unwrap();
    fs::write(library.join("bios").join("gba_bios.bin"), b"bios").unwrap();
    let card = scratch("card");

    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &[],
        Some("bios"),
    )
    .unwrap();
    let dests: Vec<_> = plan
        .items
        .iter()
        .map(|item| item.relative_dest.clone())
        .collect();

    assert_eq!(dests, vec!["gba/game.gba", "bios/gba_bios.bin"]);
    assert!(plan.warning.is_none());
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn include_list_limits_which_systems_are_copied() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::create_dir_all(library.join("snes")).unwrap();
    fs::write(library.join("gba").join("game.gba"), b"gba").unwrap();
    fs::write(library.join("snes").join("game.sfc"), b"snes").unwrap();
    let card = scratch("card");

    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &["snes".into()],
        None,
    )
    .unwrap();
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].relative_dest, "snes/game.sfc");
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn unchanged_file_is_skipped_and_dry_run_writes_nothing() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::write(library.join("gba").join("game.gba"), b"rom-bytes").unwrap();
    let card = scratch("card");
    fs::create_dir_all(card.join("gba")).unwrap();
    fs::write(card.join("gba").join("game.gba"), b"rom-bytes").unwrap();

    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &["gba".into()],
        None,
    )
    .unwrap();
    assert_eq!(plan.items.len(), 1);
    assert_eq!(plan.items[0].action, CopyAction::SkipUnchanged);

    let report = execute_copy(&plan, &card, true).unwrap();
    assert_eq!(report.copied, 0);
    assert_eq!(
        fs::read(card.join("gba").join("game.gba")).unwrap(),
        b"rom-bytes"
    );

    fs::write(library.join("gba").join("new.gba"), b"fresh").unwrap();
    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &["gba".into()],
        None,
    )
    .unwrap();
    let report = execute_copy(&plan, &card, true).unwrap();
    assert_eq!(report.copied, 1);
    assert!(!card.join("gba").join("new.gba").exists());

    let report = execute_copy(&plan, &card, false).unwrap();
    assert_eq!(report.copied, 1);
    assert_eq!(report.skipped, 1);
    assert_eq!(
        fs::read(card.join("gba").join("new.gba")).unwrap(),
        b"fresh"
    );
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn failed_copy_leaves_no_truncated_destination() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::write(library.join("gba").join("game.gba"), b"full-rom-bytes").unwrap();
    let card = scratch("card");

    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &["gba".into()],
        None,
    )
    .unwrap();
    // A directory where the temp file belongs makes the write fail the way an
    // interrupted or locked copy does.
    fs::create_dir_all(card.join("gba").join("game.gba.cfwpart")).unwrap();

    let error = execute_copy(&plan, &card, false).unwrap_err();
    assert!(error.contains("could not copy"), "{error}");
    assert!(
        !card.join("gba").join("game.gba").exists(),
        "a failed copy must not leave a destination file"
    );
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn successful_copy_replaces_and_leaves_no_temp_files() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::write(library.join("gba").join("game.gba"), b"new-rom-bytes").unwrap();
    let card = scratch("card");
    fs::create_dir_all(card.join("gba")).unwrap();
    fs::write(card.join("gba").join("game.gba"), b"old").unwrap();
    // A stale temp file from an earlier interrupted run must not block the retry.
    fs::write(card.join("gba").join("game.gba.cfwpart"), b"stale").unwrap();

    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &["gba".into()],
        None,
    )
    .unwrap();
    let report = execute_copy(&plan, &card, false).unwrap();

    assert_eq!(report.copied, 1);
    assert_eq!(
        fs::read(card.join("gba").join("game.gba")).unwrap(),
        b"new-rom-bytes"
    );
    let leftovers: Vec<_> = walk(&card)
        .into_iter()
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension.eq_ignore_ascii_case("cfwpart"))
        })
        .collect();
    assert!(
        leftovers.is_empty(),
        "temp files left behind: {leftovers:?}"
    );
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn rocknix_copy_nests_games_and_bios_under_roms() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::create_dir_all(library.join("bios")).unwrap();
    fs::write(library.join("gba").join("test.gba"), b"rom").unwrap();
    fs::write(library.join("bios").join("qa-bios.bin"), b"bios").unwrap();
    let card = scratch("card");

    let plan = plan_copy(
        &library,
        &card,
        "rocknix_roms_nested",
        &systems(),
        &["gba".into()],
        Some("bios"),
    )
    .unwrap();
    let dests: Vec<_> = plan
        .items
        .iter()
        .map(|item| item.relative_dest.clone())
        .collect();

    assert_eq!(dests, vec!["roms/gba/test.gba", "roms/bios/qa-bios.bin"]);
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn copy_second_dummy_onto_rocknix_card_when_requested() {
    let Ok(root) = std::env::var("CFW_QA_CARD") else {
        return;
    };
    let library = PathBuf::from(std::env::var("CFW_QA_LIBRARY").expect("CFW_QA_LIBRARY"));
    let card = PathBuf::from(&root);
    let existing = card.join("roms").join("gba").join("test.gba");
    assert_eq!(
        fs::read(&existing).expect("existing gba dummy"),
        b"cfw-card-studio-qa-dummy"
    );

    let plan = plan_copy(
        &library,
        &card,
        "rocknix_roms_nested",
        &systems(),
        &["gba".into()],
        Some("bios"),
    )
    .unwrap();
    let report = execute_copy(&plan, &card, false).unwrap();

    assert_eq!(report.copied, 1, "{plan:?}");
    assert_eq!(report.skipped, 1, "{plan:?}");
    assert_eq!(fs::read(&existing).unwrap(), b"cfw-card-studio-qa-dummy");
    assert_eq!(
        fs::read(card.join("roms").join("bios").join("qa-bios.bin")).unwrap(),
        b"rocknix-qa-bios"
    );
    assert!(!card.join("bios").exists());
    assert!(!card.join("gba").exists());
}

#[test]
fn stock_r36s_copy_uses_stock_folder_names() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::create_dir_all(library.join("bios")).unwrap();
    fs::write(library.join("gba").join("test.gba"), b"rom").unwrap();
    fs::write(library.join("bios").join("qa-bios.bin"), b"bios").unwrap();
    let card = scratch("card");

    let plan = plan_copy(
        &library,
        &card,
        "stock_r36s_roms",
        &systems(),
        &["gba".into()],
        Some("bios"),
    )
    .unwrap();
    let dests: Vec<_> = plan
        .items
        .iter()
        .map(|item| item.relative_dest.clone())
        .collect();

    assert_eq!(dests, vec!["Roms/GBA/test.gba", "BIOS/qa-bios.bin"]);
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn stock_r36s_copy_rejects_unmapped_system_folders() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("xu")).unwrap();
    fs::write(library.join("xu").join("game.xu"), b"rom").unwrap();
    let card = scratch("card");
    let systems = vec![SystemFolder {
        id: "xu".into(),
        folder: "xu".into(),
        extensions: vec![],
        dat_name_pattern: None,
    }];

    let error = plan_copy(&library, &card, "stock_r36s_roms", &systems, &[], None).unwrap_err();
    assert!(error.contains("no stock R36S map"), "{error}");
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn empty_library_match_sets_a_warning() {
    let library = scratch("lib");
    let card = scratch("card");
    let plan = plan_copy(
        &library,
        &card,
        "arkos_easyroms_root",
        &systems(),
        &[],
        None,
    )
    .unwrap();
    assert!(plan.items.is_empty());
    assert!(plan
        .warning
        .as_deref()
        .unwrap_or("")
        .to_lowercase()
        .contains("no"));
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);
}

#[test]
fn plan_rejects_escaping_destination_folders() {
    let library = scratch("lib");
    fs::create_dir_all(library.join("gba")).unwrap();
    fs::write(library.join("gba").join("game.gba"), b"rom").unwrap();
    let card = scratch("card");
    let systems = vec![SystemFolder {
        id: "gba".into(),
        folder: "gba".into(),
        extensions: vec![".gba".into()],
        dat_name_pattern: None,
    }];
    // Sanity: folder_map maps gba to plain "gba" for the arkos layout, so the
    // plan must refuse only a layout mapping that tries to escape.
    let plan = plan_copy(&library, &card, "arkos_easyroms_root", &systems, &[], None).unwrap();
    assert_eq!(plan.items.len(), 1);
    let _ = fs::remove_dir_all(&library);
    let _ = fs::remove_dir_all(&card);

    // A hostile mapped prefix (as a hostile feed could ship) must be refused.
    let evil = vec![SystemFolder {
        id: "gba".into(),
        folder: "..".into(),
        extensions: vec![".gba".into()],
        dat_name_pattern: None,
    }];
    let library2 = scratch("lib2");
    let card2 = scratch("card2");
    let error = plan_copy(&library2, &card2, "arkos_easyroms_root", &evil, &[], None).unwrap_err();
    assert!(error.to_lowercase().contains("unsafe"), "{error}");
    let _ = fs::remove_dir_all(&library2);
    let _ = fs::remove_dir_all(&card2);
}
