use crate::profiles::Profile;

pub fn storage_folder(layout: &str, folder: &str) -> Result<String, String> {
    match layout {
        "arkos_easyroms_root" => Ok(apply_prefix("", folder)),
        "rocknix_roms_nested" => Ok(apply_prefix("roms/", folder)),
        "stock_r36s_roms" => stock_r36s_folder(folder).ok_or_else(|| {
            format!("folder \"{folder}\" has no stock R36S map; add a recipe before seeding")
        }),
        other => Err(format!(
            "layout \"{other}\" has no folder map; add a recipe before seeding"
        )),
    }
}

// The clone's stock games partition keeps games in Roms/<NAME> with the stock
// short names and BIOS at the partition root.
fn stock_r36s_folder(folder: &str) -> Option<String> {
    let name = match folder.trim().to_ascii_lowercase().as_str() {
        "nes" => "Roms/FC",
        "snes" => "Roms/SFC",
        "md" => "Roms/MD",
        "gb" => "Roms/GB",
        "gbc" => "Roms/GBC",
        "gba" => "Roms/GBA",
        "psx" => "Roms/PS",
        "bios" => "BIOS",
        _ => return None,
    };
    Some(name.to_string())
}

pub fn folders_for(profile: &Profile) -> Result<Vec<String>, String> {
    match profile.rom_schema.layout.as_str() {
        "arkos_easyroms_root" | "rocknix_roms_nested" | "stock_r36s_roms" => {}
        other => {
            return Err(format!(
                "layout \"{other}\" has no folder map; add a recipe before seeding"
            ));
        }
    }
    let mut folders = Vec::new();
    for system in &profile.rom_schema.systems {
        push_unique(
            &mut folders,
            &storage_folder(&profile.rom_schema.layout, &system.folder)?,
        );
    }
    if let Some(bios) = &profile.rom_schema.bios_folder {
        push_unique(
            &mut folders,
            &storage_folder(&profile.rom_schema.layout, bios)?,
        );
    }
    Ok(folders)
}

fn apply_prefix(prefix: &str, folder: &str) -> String {
    let folder = folder.trim_matches('/');
    if prefix.is_empty() || folder.starts_with("roms/") || folder == "roms" {
        folder.to_string()
    } else {
        format!("{prefix}{folder}")
    }
}

fn push_unique(folders: &mut Vec<String>, folder: &str) {
    if !folder.is_empty() && !folders.iter().any(|existing| existing == folder) {
        folders.push(folder.to_string());
    }
}
