export function restoreConfirmPrompt(disk: {
  number: number;
  name: string;
  sizeLabel: string;
}): string {
  return `Type FLASH to restore onto disk ${disk.number} (${disk.name}, ${disk.sizeLabel})`;
}

/** Model text shown on the restore screen. Matches the live-disk query. */
export function restoreDiskModel(name: string): string {
  const cleaned = name.replace(/\|/g, "/").replace(/[\r\n]/g, " ").trim();
  return cleaned.length > 0 ? cleaned : "USB disk";
}

export function isFinishedBackupName(path: string): boolean {
  const trimmed = path.trim();
  const slash = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const name = (slash >= 0 ? trimmed.slice(slash + 1) : trimmed).toLowerCase();
  return name.endsWith(".img.gz") && !name.includes(".partial");
}

export function restorePhaseLabel(phase: string): string {
  if (phase === "verify") return "Checking the backup";
  if (phase === "readback") return "Reading the card back";
  return "Writing the card";
}
