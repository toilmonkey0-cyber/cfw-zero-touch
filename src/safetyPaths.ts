/**
 * Card-safety commands. Callers must not wrap these in a license check.
 * A slow license lookup does not belong on this path.
 */
export const SAFETY_COMMANDS = [
  "backup_stock_card",
  "cancel_stock_backup",
  "restore_backup",
  "cancel_restore",
  "probe_card",
  "cancel_probe",
  "full_check_card",
  "cancel_full_check",
  "read_disk_identity",
  "list_flash_disks",
  "list_probe_disks",
] as const;

export type SafetyCommand = (typeof SAFETY_COMMANDS)[number];

export type SafetyInvoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export async function invokeSafety<T>(
  invoke: SafetyInvoke,
  command: SafetyCommand,
  args?: Record<string, unknown>,
): Promise<T> {
  return invoke<T>(command, args);
}
