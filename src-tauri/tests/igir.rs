use std::fs;
use std::path::{Path, PathBuf};

use cfw_zero_touch_lib::igir::stage_plans;
use cfw_zero_touch_lib::profiles::SystemFolder;

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
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn system(id: &str, pattern: Option<&str>) -> SystemFolder {
    SystemFolder {
        id: id.into(),
        folder: id.into(),
        extensions: vec![".zip".into()],
        dat_name_pattern: pattern.map(str::to_string),
    }
}

fn systems() -> Vec<SystemFolder> {
    vec![
        system("gba", Some("^Nintendo - Game Boy Advance")),
        system("nes", Some("^Nintendo - Entertainment System")),
        system("arcade", None),
    ]
}

#[test]
fn dat_mode_adds_region_single_and_pattern_flags() {
    let library = scratch("igir-lib");
    let staging = scratch("igir-stage");
    let dat = scratch("igir-dat");

    let plans = stage_plans(
        &library,
        &staging,
        &systems(),
        &["gba".into()],
        Some(&dat),
        &["USA".into(), "EUR".into()],
        true,
    );

    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].system_id, "gba");
    assert_eq!(
        plans[0].args,
        vec![
            "copy".to_string(),
            "--input".to_string(),
            format!(
                "{}/gba/**",
                library.display().to_string().replace('\\', "/")
            ),
            "--output".to_string(),
            format!("{}/gba/", staging.display().to_string().replace('\\', "/")),
            "--overwrite-invalid".to_string(),
            "--dat".to_string(),
            format!("{}/**", dat.display().to_string().replace('\\', "/")),
            "--dat-name-regex".to_string(),
            "^Nintendo - Game Boy Advance".to_string(),
            "--filter-region".to_string(),
            "USA,EUR".to_string(),
            "--single".to_string(),
        ]
    );
    let _ = std::fs::remove_dir_all(&library);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&dat);
}

#[test]
fn without_a_dat_folder_everything_stages_by_plain_copy() {
    let library = scratch("igir-lib");
    let staging = scratch("igir-stage");

    let plans = stage_plans(
        &library,
        &staging,
        &systems(),
        &[],
        None,
        &["USA".into()],
        true,
    );

    assert_eq!(plans.len(), 3, "empty include keeps every system");
    for plan in &plans {
        assert!(
            !plan.args.iter().any(|arg| arg == "--dat"),
            "no DAT flags without a DAT folder: {plan:?}"
        );
        assert!(
            !plan.args.iter().any(|arg| arg == "--single"),
            "1G1R needs a DAT: {plan:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&library);
    let _ = std::fs::remove_dir_all(&staging);
}

#[test]
fn patternless_systems_stage_raw_even_with_a_dat() {
    let library = scratch("igir-lib");
    let staging = scratch("igir-stage");
    let dat = scratch("igir-dat");

    let plans = stage_plans(
        &library,
        &staging,
        &systems(),
        &["arcade".into()],
        Some(&dat),
        &["USA".into()],
        true,
    );

    assert_eq!(plans.len(), 1);
    assert_eq!(plans[0].system_id, "arcade");
    assert!(!plans[0].args.iter().any(|arg| arg == "--dat"));
    let _ = std::fs::remove_dir_all(&library);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&dat);
}

#[test]
fn include_list_limits_systems_and_empty_regions_omit_the_flag() {
    let library = scratch("igir-lib");
    let staging = scratch("igir-stage");
    let dat = scratch("igir-dat");

    let plans = stage_plans(
        &library,
        &staging,
        &systems(),
        &["gba".into(), "nes".into()],
        Some(&dat),
        &[],
        false,
    );

    let ids: Vec<_> = plans.iter().map(|plan| plan.system_id.as_str()).collect();
    assert_eq!(ids, vec!["gba", "nes"]);
    let gba = &plans[0];
    assert!(!gba.args.iter().any(|arg| arg == "--filter-region"));
    assert!(!gba.args.iter().any(|arg| arg == "--single"));
    let _ = std::fs::remove_dir_all(&library);
    let _ = std::fs::remove_dir_all(&staging);
    let _ = std::fs::remove_dir_all(&dat);
}

#[test]
fn one_game_per_region_stages_against_a_self_made_dat() {
    let Ok(_) = std::env::var("CFW_IGIR_TEST") else {
        return;
    };
    let root = scratch("igir-int");
    let library = root.join("lib");
    let dat_dir = root.join("dat");
    let staging = root.join("stage");
    let nes_dir = library.join("nes");
    fs::create_dir_all(&nes_dir).unwrap();
    fs::create_dir_all(&dat_dir).unwrap();

    let usa = nes_dir.join("Game (USA).nes");
    let europe = nes_dir.join("Game (Europe).nes");
    let other = nes_dir.join("Other (USA).nes");
    fs::write(&usa, b"fixture-rom-content-v1").unwrap();
    fs::write(&europe, b"fixture-rom-content-v1").unwrap();
    fs::write(&other, b"other-fixture-content-v1").unwrap();

    // Hand-written Logiqx DAT: the Europe game is a clone of the USA parent.
    fs::write(
        dat_dir.join("nes.dat"),
        r#"<?xml version="1.0"?>
<datafile>
  <header>
    <name>Nintendo - Entertainment System</name>
  </header>
  <game name="Game (USA)">
    <rom name="Game (USA).nes" size="22" md5="3e44c77f4e388ef0cc974c9397c55b03" sha1="d1db6f9822cb9448b8e5460316e1f77e6c7e8b5d"/>
  </game>
  <game name="Game (Europe)" cloneof="Game (USA)">
    <rom name="Game (Europe).nes" size="22" md5="3e44c77f4e388ef0cc974c9397c55b03" sha1="d1db6f9822cb9448b8e5460316e1f77e6c7e8b5d"/>
  </game>
  <game name="Other Game (USA)">
    <rom name="Other (USA).nes" size="24" md5="cbaa1532a7262ba350db41341fe0f4bd" sha1="558b668893cc30af46fd6f3e234e2cb0117f5467"/>
  </game>
</datafile>
"#,
    )
    .unwrap();

    let systems = vec![system("nes", Some(".*"))];
    let plans = stage_plans(
        &library,
        &staging,
        &systems,
        &[],
        Some(&dat_dir),
        &["USA".into()],
        true,
    );

    // Spawn igir directly: the app's spawn wrapper lives behind tauri's
    // Emitter, which bare test binaries cannot load on Windows.
    let on_path = |name: &str| {
        std::process::Command::new("where")
            .arg(name)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false)
    };
    let base = if on_path("igir.exe") {
        vec!["igir".to_string()]
    } else if on_path("npx.cmd") {
        vec![
            "npx.cmd".to_string(),
            "--yes".to_string(),
            "igir@latest".to_string(),
        ]
    } else {
        vec![
            "npx".to_string(),
            "--yes".to_string(),
            "igir@latest".to_string(),
        ]
    };
    for plan in &plans {
        let mut command = std::process::Command::new(&base[0]);
        command.args(&base[1..]).args(&plan.args);
        let output = command.output().expect("igir must run for this test");
        assert!(
            output.status.success(),
            "igir failed for {}: {}",
            plan.system_folder,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    assert_eq!(count_files(&staging), 2, "one game per title");
    assert!(
        staging.join("nes").join("Game (USA).nes").is_file(),
        "the USA copy must win"
    );
    assert!(
        !staging.join("nes").join("Game (Europe).nes").exists(),
        "the Europe duplicate must not stage under 1G1R + USA"
    );
    assert!(staging.join("nes").join("Other (USA).nes").is_file());
    let _ = fs::remove_dir_all(&root);
}

fn count_files(root: &Path) -> usize {
    let mut count = 0;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                count += 1;
            }
        }
    }
    count
}
