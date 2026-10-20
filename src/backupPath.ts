export function stockImagePath(path: string): string {
  const trimmed = path.trim();
  if (trimmed.endsWith(".img.gz")) {
    return trimmed;
  }
  if (trimmed.endsWith(".gz")) {
    const withoutGz = trimmed.slice(0, -3);
    return withoutGz.endsWith(".img") ? trimmed : `${withoutGz}.img.gz`;
  }
  return `${trimmed}.img.gz`;
}
