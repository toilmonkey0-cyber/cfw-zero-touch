use crate::volume::VolumeInfo;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormatRequest {
    pub volume_id: String,
    pub letter: String,
    pub displayed_bytes: u64,
    pub confirmation: String,
    pub file_system: String,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormatBlock {
    Confirmation,
    NotRemovable,
    Identity,
    Size,
    Script,
}

pub fn authorize_format(current: &VolumeInfo, request: &FormatRequest) -> Result<(), FormatBlock> {
    if request.confirmation != "FORMAT" {
        return Err(FormatBlock::Confirmation);
    }
    if !current.is_removable {
        return Err(FormatBlock::NotRemovable);
    }
    if current.total_bytes != request.displayed_bytes {
        return Err(FormatBlock::Size);
    }
    if current.id != request.volume_id || current.letter != request.letter {
        return Err(FormatBlock::Identity);
    }
    format_script(&request.letter, &request.file_system, &request.label)
        .map(|_| ())
        .map_err(|_| FormatBlock::Script)
}

pub fn format_script(letter: &str, file_system: &str, label: &str) -> Result<String, String> {
    let letter = single_letter(letter)?;
    let file_system = match file_system.trim().to_ascii_uppercase().as_str() {
        "EXFAT" => "exFAT",
        "FAT32" => "FAT32",
        other => return Err(format!("unsupported file system {other}")),
    };
    if !is_safe_label(label) {
        return Err(
            "volume label must be 1-11 letters, digits, spaces, hyphens, or underscores".into(),
        );
    }
    Ok(format!(
        "Format-Volume -DriveLetter {letter} -FileSystem {file_system} -NewFileSystemLabel '{label}' -Confirm:$false -Force"
    ))
}

pub fn relabel_script(letter: &str, label: &str) -> Result<String, String> {
    let letter = single_letter(letter)?;
    if !is_safe_label(label) {
        return Err(
            "volume label must be 1-11 letters, digits, spaces, hyphens, or underscores".into(),
        );
    }
    Ok(format!(
        "Set-Volume -DriveLetter {letter} -NewFileSystemLabel '{label}'"
    ))
}

fn single_letter(letter: &str) -> Result<char, String> {
    let mut chars = letter.chars();
    match (chars.next(), chars.next()) {
        (Some(ch), None) if ch.is_ascii_alphabetic() => Ok(ch.to_ascii_uppercase()),
        _ => Err("drive letter must be a single A-Z character".into()),
    }
}

fn is_safe_label(label: &str) -> bool {
    let len = label.chars().count();
    (1..=11).contains(&len)
        && label
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == ' ' || ch == '-' || ch == '_')
}
