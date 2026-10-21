export function probeConfirmPrompt(disk: {
  number: number;
  name: string;
  sizeLabel: string;
}): string {
  return `Type PROBE to test disk ${disk.number} (${disk.name}, ${disk.sizeLabel})`;
}
