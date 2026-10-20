// Byte labels shown next to disks, volumes, copy reports, and the smart-sort engine.
export function formatBytes(bytes: number): string {
  if (bytes < 0) return "unknown size";
  if (bytes < 1000) return `${bytes} B`;
  const mb = bytes / 1_000_000;
  if (mb < 1000) return mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
}
