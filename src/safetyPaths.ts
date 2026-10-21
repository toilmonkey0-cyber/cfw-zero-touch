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

export type SafetyReport = {
  diskZero: string;
  identity: { serial: string; partitionTableId: string; sizeBytes: number };
  probe: { overwritten: boolean; kind: string; summary: string };
  backup: { status: string; image: string };
};

/** Identity refusal, a removable-card read, the fake-card probe, and a backup. */
export async function runSafetyPaths(invoke: SafetyInvoke): Promise<SafetyReport> {
  let diskZero = "";
  try {
    await invokeSafety(invoke, "read_disk_identity", { diskNumber: 0 });
  } catch (cause) {
    diskZero = cause instanceof Error ? cause.message : String(cause);
  }
  const identity = await invokeSafety<{
    serial: string;
    partitionTableId: string;
    sizeBytes: number;
  }>(invoke, "read_disk_identity", { diskNumber: 2 });
  const probe = await invokeSafety<{ overwritten: boolean; kind: string; summary: string }>(
    invoke,
    "probe_card",
    { diskNumber: 2 },
  );
  const backup = await invokeSafety<{ status: string; image: string }>(invoke, "backup_stock_card", {
    diskNumber: 2,
  });
  return { diskZero, identity, probe, backup };
}
