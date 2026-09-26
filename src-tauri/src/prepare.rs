use std::fs;
use std::process::Command;

use crate::format_gate::{authorize_format, format_script, relabel_script, FormatRequest};
use crate::volume::{decide, VolumeDecision, VolumeInfo};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrepareOutcome {
    AlreadyReady,
    Relabeled,
    Formatted,
}

pub trait Shell {
    fn run(&mut self, script: &str) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptCall {
    pub script: String,
}

#[derive(Debug, Default)]
pub struct ShellLog {
    pub calls: Vec<ScriptCall>,
}

impl Shell for ShellLog {
    fn run(&mut self, script: &str) -> Result<(), String> {
        self.calls.push(ScriptCall {
            script: script.to_string(),
        });
        Ok(())
    }
}

pub struct ElevatedPowerShell;

impl Shell for ElevatedPowerShell {
    fn run(&mut self, script: &str) -> Result<(), String> {
        run_elevated_powershell(script)
    }
}

pub fn run_elevated_powershell(script: &str) -> Result<(), String> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!("cfw-prepare-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&dir).map_err(|error| format!("could not create a temp script: {error}"))?;
    let script_path = dir.join("prepare.ps1");
    let result_path = dir.join("result.txt");
    let result_lit = result_path.display().to_string().replace('\'', "''");
    let body = format!(
        "$ErrorActionPreference = 'Stop'\r\ntry {{\r\n{script}\r\n'Stop-ok' | Set-Content -LiteralPath '{result_lit}' -Encoding utf8\r\n}} catch {{\r\n$_ | Out-String | Set-Content -LiteralPath '{result_lit}' -Encoding utf8\r\nexit 1\r\n}}\r\n"
    );
    fs::write(&script_path, body)
        .map_err(|error| format!("could not write the prepare script: {error}"))?;
    let script_arg = script_path.display().to_string().replace('\'', "''");
    let launcher = format!(
        "Start-Process -FilePath powershell.exe -Verb RunAs -Wait -ArgumentList '-NoProfile','-ExecutionPolicy','Bypass','-File','{script_arg}'"
    );
    let status = Command::new("powershell.exe")
        .args(["-NoProfile", "-Command", &launcher])
        .status()
        .map_err(|error| format!("could not start PowerShell: {error}"))?;
    let text = fs::read_to_string(&result_path).unwrap_or_default();
    let _ = fs::remove_dir_all(&dir);
    let text = text.trim().trim_start_matches('\u{feff}');
    if text == "Stop-ok" {
        return Ok(());
    }
    if !status.success() {
        return Err(
            "Windows did not run the prepare step. The approval prompt may have been dismissed."
                .into(),
        );
    }
    if text.is_empty() {
        Err("the prepare step did not report success".into())
    } else {
        Err(text.to_string())
    }
}

pub fn prepare_volume(
    volume: &VolumeInfo,
    expected_label: &str,
    confirmation: &str,
    displayed_bytes: u64,
    file_system: &str,
    shell: &mut impl Shell,
) -> Result<PrepareOutcome, String> {
    match decide(volume, expected_label) {
        VolumeDecision::Rejected { reason } => Err(reason),
        VolumeDecision::Ready { relabel: false } => Ok(PrepareOutcome::AlreadyReady),
        VolumeDecision::Ready { relabel: true } => {
            let script = relabel_script(&volume.letter, expected_label)?;
            shell.run(&script)?;
            Ok(PrepareOutcome::Relabeled)
        }
        VolumeDecision::NeedsFormat { reason } => {
            if confirmation != "FORMAT" {
                return Err(format!(
                    "this card needs a format ({reason}). Type FORMAT to erase it"
                ));
            }
            let file_system = resolve_format_fs(file_system, volume.total_bytes)?;
            let request = FormatRequest {
                volume_id: volume.id.clone(),
                letter: volume.letter.clone(),
                displayed_bytes,
                confirmation: confirmation.to_string(),
                file_system: file_system.to_string(),
                label: expected_label.to_string(),
            };
            authorize_format(volume, &request)
                .map_err(|block| format!("format blocked: {block:?}"))?;
            let script = format_script(&volume.letter, file_system, expected_label)?;
            shell.run(&script)?;
            Ok(PrepareOutcome::Formatted)
        }
    }
}

// Windows refuses to format FAT32 on volumes over 32 GB, so a stock-layout card
// must present a partition of 32 GiB or less.
fn resolve_format_fs(requested: &str, total_bytes: u64) -> Result<&'static str, String> {
    const FAT32_MAX_BYTES: u64 = 32 * 1024 * 1024 * 1024;
    match requested.trim().to_ascii_uppercase().as_str() {
        "" | "EXFAT" => Ok("exFAT"),
        "FAT32" => {
            if total_bytes > FAT32_MAX_BYTES {
                Err(format!(
                    "Windows cannot format FAT32 on a volume over 32 GB; this volume is {:.1} GB. Use a card partition of 32 GB or less.",
                    total_bytes as f64 / 1_000_000_000.0
                ))
            } else {
                Ok("FAT32")
            }
        }
        other => Err(format!("unsupported format file system {other}")),
    }
}
