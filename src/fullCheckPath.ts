export function fullConfirmPrompt(disk: {
  number: number;
  name: string;
  sizeLabel: string;
}): string {
  return `Type FULL to erase and test disk ${disk.number} (${disk.name}, ${disk.sizeLabel})`;
}

export function fullProgressLabel(progress: {
  phase: string;
  percent: number;
  mbps: number;
  etaSeconds: number;
}): string {
  const phase = progress.phase === "read" ? "Reading" : "Writing";
  const speed = Number.isFinite(progress.mbps)
    ? `${progress.mbps.toFixed(1)} MB/s`
    : "unknown speed";
  return `${phase} ${progress.percent}% · ${speed} · ${etaPhrase(progress.etaSeconds)}`;
}

function etaPhrase(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds <= 0) return "finishing";
  if (seconds < 60) return `${Math.ceil(seconds)}s left`;
  const minutes = Math.ceil(seconds / 60);
  return minutes === 1 ? "1 min left" : `${minutes} min left`;
}
