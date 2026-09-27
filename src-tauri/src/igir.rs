use std::path::Path;

use crate::profiles::SystemFolder;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagePlan {
    pub system_id: String,
    pub system_folder: String,
    pub args: Vec<String>,
}

fn igir_path(path: &Path) -> String {
    path.display().to_string().replace('\\', "/")
}

/// One igir `copy` invocation per included system. Systems with both a DAT
/// folder and a `datNamePattern` get 1G1R/region/verify flags; everything
/// else stages by plain copy. igir never cleans and never touches the card.
pub fn stage_plans(
    library: &Path,
    staging: &Path,
    systems: &[SystemFolder],
    include: &[String],
    dat_folder: Option<&Path>,
    regions: &[String],
    single: bool,
) -> Vec<StagePlan> {
    let mut plans = Vec::new();
    for system in systems {
        if !include.is_empty() && !include.iter().any(|id| id.eq_ignore_ascii_case(&system.id)) {
            continue;
        }
        // Load-time validation (profiles::is_safe_folder) already rejects
        // unsafe folders; skip defensively so staging can never be aimed
        // outside the staging directory.
        if !crate::profiles::is_safe_folder(&system.folder) {
            continue;
        }
        let mut args = vec![
            "copy".to_string(),
            "--input".to_string(),
            format!(
                "{}/{}/**",
                igir_path(library),
                igir_path(Path::new(&system.folder))
            ),
            "--output".to_string(),
            format!("{}/", igir_path(&staging.join(&system.folder))),
            "--overwrite-invalid".to_string(),
        ];
        if let (Some(dat), Some(pattern)) = (dat_folder, system.dat_name_pattern.as_deref()) {
            args.push("--dat".into());
            args.push(format!("{}/**", igir_path(dat)));
            args.push("--dat-name-regex".into());
            args.push(pattern.to_string());
            if !regions.is_empty() {
                args.push("--filter-region".into());
                args.push(regions.join(","));
            }
            if single {
                args.push("--single".into());
            }
        }
        plans.push(StagePlan {
            system_id: system.id.clone(),
            system_folder: system.folder.clone(),
            args,
        });
    }
    plans
}
