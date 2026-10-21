import { detectionNotice, type StockDetection } from "./detectDevice";
import { isFinishedBackupName } from "./restorePath";

/**
 * Guided setup decides which existing command may run next.
 * It does not open, format, flash, or copy to a disk. Callers pass the
 * user's typed confirmation through; this module never supplies one.
 */

export const GUIDED_STEPS = [
  "backup",
  "probe",
  "fullCheck",
  "detect",
  "flash",
  "bootOnce",
  "copy",
] as const;

export type GuidedStep = (typeof GUIDED_STEPS)[number];

export type CommandName =
  | "backup_stock_card"
  | "probe_card"
  | "full_check_card"
  | "detect_stock_device"
  | "flash_os"
  | "restore_backup"
  | "copy_roms";

export const STEP_COMMAND: Record<GuidedStep, CommandName | null> = {
  backup: "backup_stock_card",
  probe: "probe_card",
  fullCheck: "full_check_card",
  detect: "detect_stock_device",
  flash: "flash_os",
  bootOnce: null,
  copy: "copy_roms",
};

export const STEP_TITLES: Record<GuidedStep, string> = {
  backup: "Back up the card",
  probe: "Quick card test",
  fullCheck: "Full card test",
  detect: "Check which handheld this is",
  flash: "Flash the system",
  bootOnce: "Boot the handheld once",
  copy: "Copy your games",
};

export const BOOT_ONCE_STEPS = [
  "Eject the card and put it in the handheld.",
  "Turn the handheld on and wait until the game menu appears.",
  "Shut down from the menu.",
  "Put the card back in this computer.",
];

export const SKIP_WARNINGS: Record<GuidedStep, string> = {
  backup:
    "Skipping the backup means this card cannot be restored. The backup route stays off. The blank route still applies to a card that is empty.",
  probe:
    "Skipping the quick test means a card that claims the wrong size can still be erased by a later step.",
  fullCheck:
    "Skipping the full check leaves bad spots on the card untested. You can still go on.",
  detect:
    "Skipping detection means nothing is pre-selected. You still have to choose the handheld yourself.",
  flash: "Skipping the flash leaves the card's current system in place.",
  bootOnce:
    "Skipping this step does not boot the handheld. Copy stays off until you put the card back and the card is recognized again. A first boot can erase games copied too early.",
  copy: "Skipping the copy leaves your games off the card.",
};

const STEP_NAME: Record<GuidedStep, string> = {
  backup: "backup",
  probe: "quick test",
  fullCheck: "full check",
  detect: "device detection",
  flash: "flash",
  bootOnce: "boot",
  copy: "copy",
};

const WORD: Partial<Record<CommandName, string>> = {
  probe_card: "PROBE",
  full_check_card: "FULL",
  flash_os: "FLASH",
  restore_backup: "FLASH",
};

export type FlowCall = {
  command: CommandName;
  confirmation: string;
  route: "" | "backup" | "blank";
  diskNumber: number;
  profileId: string;
  volumeId: string;
  destination: string;
  imagePath: string;
};

export type Plan = { ok: true; call: FlowCall } | { ok: false; reason: string };

export type GuidedFlow = {
  step: GuidedStep | "done";
  stopped: boolean;
  status: string;
  note: string;
  warnings: string[];
  backupDone: boolean;
  backupRouteDisabled: boolean;
  suggestedProfileId: string;
  confirmedProfileId: string;
  detectionMessage: string;
  cardReinserted: boolean;
  identityRechecked: boolean;
  redetectedProfileId: string;
  calls: FlowCall[];
};

export type BackupOutcome = {
  status: "complete" | "cancelled" | "failed";
  summary: string;
};

export type ProbeOutcome = {
  status: "finished" | "cancelled" | "failed";
  kind: string;
  overwritten: boolean | null;
  summary: string;
};

export type FullOutcome = {
  status: "finished" | "cancelled" | "failed";
  badCount: number;
  overwritten: boolean | null;
  summary: string;
};

export type WriteOutcome =
  | { ok: true; summary: string }
  | {
      ok: false;
      summary: string;
      overwritten: boolean | null;
      outcome: "failed" | "cancelled";
    };

export type IssuedFailure = {
  outcome: "failed" | "cancelled";
  overwritten: boolean | null;
  summary: string;
};

export function startGuidedFlow(): GuidedFlow {
  return {
    step: "backup",
    stopped: false,
    status: "",
    note: "",
    warnings: [],
    backupDone: false,
    backupRouteDisabled: true,
    suggestedProfileId: "",
    confirmedProfileId: "",
    detectionMessage: "",
    cardReinserted: false,
    identityRechecked: false,
    redetectedProfileId: "",
    calls: [],
  };
}

export function skipWarning(flow: GuidedFlow): string {
  if (flow.stopped || flow.step === "done") return "";
  return SKIP_WARNINGS[flow.step];
}

export function probeRouteFlag(flow: GuidedFlow): string {
  if (flow.backupDone && !flow.backupRouteDisabled) {
    return "The backup route is on: this setup has a backup of this card. The blank route still applies if the card is empty.";
  }
  return "The backup route is off: this card cannot be restored. The blank route still applies if the card is empty.";
}

export function skipStep(flow: GuidedFlow): GuidedFlow {
  if (flow.stopped || flow.step === "done") return flow;
  const step = flow.step;
  const next: GuidedFlow = {
    ...flow,
    warnings: [...flow.warnings, SKIP_WARNINGS[step]],
    step: nextStep(step),
    note: "",
  };
  if (step === "backup") {
    return { ...next, backupDone: false, backupRouteDisabled: true };
  }
  if (step === "detect") {
    return { ...next, suggestedProfileId: "", detectionMessage: "" };
  }
  return next;
}

export function planBackup(
  flow: GuidedFlow,
  input: { diskNumber: number; destination: string },
): Plan {
  const blocked = refuseActive(flow, "backup") ?? refuseDisk(input.diskNumber) ?? refuseBackupName(input.destination);
  if (blocked) return { ok: false, reason: blocked };
  return okCall({
    command: "backup_stock_card",
    diskNumber: input.diskNumber,
    destination: input.destination.trim(),
  });
}

export function planProbe(
  flow: GuidedFlow,
  input: { diskNumber: number; confirmation: string; route: "backup" | "blank" },
): Plan {
  const blocked =
    refuseActive(flow, "probe") ??
    refuseDisk(input.diskNumber) ??
    refuseWord("probe_card", input.confirmation) ??
    refuseRoute(flow, input.route);
  if (blocked) return { ok: false, reason: blocked };
  return okCall({
    command: "probe_card",
    diskNumber: input.diskNumber,
    confirmation: input.confirmation,
    route: input.route,
  });
}

export function planFullCheck(
  flow: GuidedFlow,
  input: { diskNumber: number; confirmation: string; route: "backup" | "blank" },
): Plan {
  const blocked =
    refuseActive(flow, "fullCheck") ??
    refuseDisk(input.diskNumber) ??
    refuseWord("full_check_card", input.confirmation) ??
    refuseRoute(flow, input.route);
  if (blocked) return { ok: false, reason: blocked };
  return okCall({
    command: "full_check_card",
    diskNumber: input.diskNumber,
    confirmation: input.confirmation,
    route: input.route,
  });
}

export function planDetect(flow: GuidedFlow): Plan {
  if (flow.stopped) return { ok: false, reason: flow.status || "This setup has stopped." };
  if (flow.step === "detect") return okCall({ command: "detect_stock_device" });
  if (
    flow.cardReinserted &&
    !flow.identityRechecked &&
    (flow.step === "bootOnce" || flow.step === "copy")
  ) {
    return okCall({ command: "detect_stock_device" });
  }
  return { ok: false, reason: "Device detection is not the current step." };
}

export function planFlash(
  flow: GuidedFlow,
  input: { diskNumber: number; confirmation: string },
): Plan {
  const blocked =
    refuseActive(flow, "flash") ??
    refuseDisk(input.diskNumber) ??
    refuseWord("flash_os", input.confirmation);
  if (blocked) return { ok: false, reason: blocked };
  if (flow.confirmedProfileId.length === 0) {
    return {
      ok: false,
      reason: "Choose the handheld before flashing. A suggestion is not a confirmation.",
    };
  }
  return okCall({
    command: "flash_os",
    diskNumber: input.diskNumber,
    confirmation: input.confirmation,
    profileId: flow.confirmedProfileId,
  });
}

export function planCopy(flow: GuidedFlow, input: { volumeId: string }): Plan {
  if (flow.stopped) return { ok: false, reason: flow.status || "This setup has stopped." };
  if (!flow.cardReinserted) {
    return { ok: false, reason: "Copy stays off until you put the card back in this computer." };
  }
  if (!flow.identityRechecked) {
    return { ok: false, reason: "Copy stays off until the card is recognized again." };
  }
  if (flow.step !== "copy") return { ok: false, reason: "This step is not the current one." };
  if (
    flow.redetectedProfileId.length > 0 &&
    flow.redetectedProfileId !== flow.confirmedProfileId
  ) {
    return {
      ok: false,
      reason: "This card looks like a different handheld than the one you confirmed.",
    };
  }
  if (flow.confirmedProfileId.length === 0) {
    return { ok: false, reason: "Choose the handheld before copying." };
  }
  if (input.volumeId.trim().length === 0) {
    return { ok: false, reason: "The card is not ready to receive games." };
  }
  return okCall({
    command: "copy_roms",
    volumeId: input.volumeId.trim(),
    profileId: flow.confirmedProfileId,
  });
}

export function planRestore(
  flow: GuidedFlow,
  input: { diskNumber: number; confirmation: string; imagePath: string },
): Plan {
  if (!flow.stopped) {
    return {
      ok: false,
      reason: "Restore waits until this setup stops. It does not run in the middle of the steps.",
    };
  }
  const blocked = refuseDisk(input.diskNumber) ?? refuseWord("restore_backup", input.confirmation);
  if (blocked) return { ok: false, reason: blocked };
  if (!isFinishedBackupName(input.imagePath)) {
    return { ok: false, reason: "Choose a finished .img.gz backup. Partial backups are refused." };
  }
  return okCall({
    command: "restore_backup",
    diskNumber: input.diskNumber,
    confirmation: input.confirmation,
    imagePath: input.imagePath.trim(),
  });
}

export function finishBackup(flow: GuidedFlow, call: FlowCall, result: BackupOutcome): GuidedFlow {
  if (!matches(flow, call)) return flow;
  if (result.status === "complete") {
    return {
      ...flow,
      calls: [...flow.calls, call],
      step: "probe",
      backupDone: true,
      backupRouteDisabled: false,
      note: result.summary,
    };
  }
  return halt(flow, call, result.status === "cancelled" ? "cancelled" : "failed", false, result.summary);
}

export function finishProbe(flow: GuidedFlow, call: FlowCall, result: ProbeOutcome): GuidedFlow {
  if (!matches(flow, call)) return flow;
  if (result.status === "finished" && result.kind === "genuine") {
    return {
      ...flow,
      calls: [...flow.calls, call],
      step: "fullCheck",
      note: result.summary,
    };
  }
  const outcome = result.status === "cancelled" ? "cancelled" : "failed";
  return halt(flow, call, outcome, overwriteFlag(call.command, result.overwritten), result.summary);
}

export function finishFullCheck(flow: GuidedFlow, call: FlowCall, result: FullOutcome): GuidedFlow {
  if (!matches(flow, call)) return flow;
  if (result.status === "finished" && result.badCount === 0) {
    return {
      ...flow,
      calls: [...flow.calls, call],
      step: "detect",
      note: result.summary,
    };
  }
  const outcome = result.status === "cancelled" ? "cancelled" : "failed";
  return halt(flow, call, outcome, overwriteFlag(call.command, result.overwritten), result.summary);
}

export function applyDetection(
  flow: GuidedFlow,
  call: FlowCall,
  detection: StockDetection,
): GuidedFlow {
  if (!matches(flow, call)) return flow;
  const recorded: GuidedFlow = { ...flow, calls: [...flow.calls, call] };
  if (recheckDue(flow)) return applyRecheck(recorded, detection);
  const notice = detectionNotice(detection, false);
  return {
    ...recorded,
    suggestedProfileId: notice.suggestedProfileId,
    detectionMessage: notice.message,
    note: notice.message,
  };
}

export function failIssued(flow: GuidedFlow, call: FlowCall, failure: IssuedFailure): GuidedFlow {
  if (!matches(flow, call)) return flow;
  return halt(
    flow,
    call,
    failure.outcome,
    overwriteFlag(call.command, failure.overwritten),
    failure.summary,
  );
}

export function confirmSuggestedDevice(flow: GuidedFlow): GuidedFlow {
  if (flow.stopped || flow.step !== "detect" || flow.suggestedProfileId.length === 0) return flow;
  return {
    ...flow,
    confirmedProfileId: flow.suggestedProfileId,
    step: "flash",
    note: "",
  };
}

export function chooseDevice(flow: GuidedFlow, profileId: string): GuidedFlow {
  const id = profileId.trim();
  if (flow.stopped || id.length === 0) return flow;
  if (flow.step !== "detect" && flow.step !== "flash") return flow;
  return { ...flow, confirmedProfileId: id, step: "flash" };
}

export function finishFlash(flow: GuidedFlow, call: FlowCall, result: WriteOutcome): GuidedFlow {
  if (!matches(flow, call)) return flow;
  if (result.ok) {
    return { ...flow, calls: [...flow.calls, call], step: "bootOnce", note: result.summary };
  }
  return halt(flow, call, result.outcome, result.overwritten, result.summary);
}

export function markCardReturned(flow: GuidedFlow): GuidedFlow {
  if (flow.stopped) return flow;
  if (flow.step !== "bootOnce" && flow.step !== "copy") return flow;
  if (flow.cardReinserted) return flow;
  return { ...flow, cardReinserted: true };
}

/** Clears a finished recheck so the user can insert the card and detect it again. */
export function repeatIdentityCheck(flow: GuidedFlow): GuidedFlow {
  if (flow.stopped || flow.step !== "copy" || !flow.cardReinserted) return flow;
  return { ...flow, identityRechecked: false, redetectedProfileId: "" };
}

export function finishCopy(flow: GuidedFlow, call: FlowCall, result: WriteOutcome): GuidedFlow {
  if (!matches(flow, call)) return flow;
  if (result.ok) {
    return {
      ...flow,
      calls: [...flow.calls, call],
      step: "done",
      note: result.summary,
      status: "Guided setup finished.",
    };
  }
  return halt(flow, call, result.outcome, result.overwritten, result.summary);
}

export function finishRestore(flow: GuidedFlow, call: FlowCall, result: WriteOutcome): GuidedFlow {
  if (!matches(flow, call)) return flow;
  const recorded: GuidedFlow = { ...flow, calls: [...flow.calls, call], note: result.summary };
  if (result.ok) {
    return {
      ...recorded,
      stopped: true,
      status: `Restore finished. This setup will not flash or copy. ${flow.status}`.trim(),
    };
  }
  const step = flow.step === "done" ? "copy" : flow.step;
  return {
    ...recorded,
    stopped: true,
    status: honestStatus(step, result.outcome, result.overwritten, result.summary),
  };
}

function applyRecheck(flow: GuidedFlow, detection: StockDetection): GuidedFlow {
  const profileId = detection.status === "matched" ? detection.profileId : "";
  if (profileId.length > 0 && profileId !== flow.confirmedProfileId) {
    return {
      ...flow,
      stopped: true,
      identityRechecked: true,
      redetectedProfileId: profileId,
      note: detectionNotice(detection, false).message,
      status:
        "Stopped. This card looks like a different handheld than the one you confirmed. Copy stays off.",
    };
  }
  return {
    ...flow,
    step: "copy",
    identityRechecked: true,
    redetectedProfileId: profileId,
    note:
      profileId.length > 0
        ? detectionNotice(detection, false).message
        : "The card is back. Stock-card detection did not name a different handheld.",
  };
}

function halt(
  flow: GuidedFlow,
  call: FlowCall,
  outcome: "failed" | "cancelled",
  overwritten: boolean | null,
  summary: string,
): GuidedFlow {
  const step = flow.step === "done" ? "copy" : flow.step;
  return {
    ...flow,
    calls: [...flow.calls, call],
    stopped: true,
    step,
    note: summary,
    status: honestStatus(step, outcome, overwritten, summary),
  };
}

function honestStatus(
  step: GuidedStep,
  outcome: "failed" | "cancelled",
  overwritten: boolean | null,
  summary: string,
): string {
  const lead =
    outcome === "cancelled"
      ? `Stopped at the ${STEP_NAME[step]}. Cancelled.`
      : `Stopped at the ${STEP_NAME[step]}. Failed.`;
  const detail = summary.trim();
  if (overwritten === true) return `${lead} OVERWRITTEN. ${detail}`.trim();
  if (overwritten === false) {
    return `${lead} Nothing on the card was overwritten. ${detail}`.trim();
  }
  return `${lead} The result did not say whether the card was overwritten. ${detail}`.trim();
}

function overwriteFlag(command: CommandName, overwritten: boolean | null): boolean | null {
  if (command === "backup_stock_card" || command === "detect_stock_device") return false;
  return overwritten;
}

function matches(flow: GuidedFlow, call: FlowCall): boolean {
  const planned = replan(flow, call);
  return planned.ok && sameCall(planned.call, call);
}

function replan(flow: GuidedFlow, call: FlowCall): Plan {
  switch (call.command) {
    case "backup_stock_card":
      return planBackup(flow, { diskNumber: call.diskNumber, destination: call.destination });
    case "probe_card":
      if (call.route !== "backup" && call.route !== "blank") {
        return { ok: false, reason: "Choose the backup route or the blank route." };
      }
      return planProbe(flow, {
        diskNumber: call.diskNumber,
        confirmation: call.confirmation,
        route: call.route,
      });
    case "full_check_card":
      if (call.route !== "backup" && call.route !== "blank") {
        return { ok: false, reason: "Choose the backup route or the blank route." };
      }
      return planFullCheck(flow, {
        diskNumber: call.diskNumber,
        confirmation: call.confirmation,
        route: call.route,
      });
    case "detect_stock_device":
      return planDetect(flow);
    case "flash_os":
      return planFlash(flow, { diskNumber: call.diskNumber, confirmation: call.confirmation });
    case "restore_backup":
      return planRestore(flow, {
        diskNumber: call.diskNumber,
        confirmation: call.confirmation,
        imagePath: call.imagePath,
      });
    case "copy_roms":
      return planCopy(flow, { volumeId: call.volumeId });
  }
}

function sameCall(left: FlowCall, right: FlowCall): boolean {
  return (
    left.command === right.command &&
    left.confirmation === right.confirmation &&
    left.route === right.route &&
    left.diskNumber === right.diskNumber &&
    left.profileId === right.profileId &&
    left.volumeId === right.volumeId &&
    left.destination === right.destination &&
    left.imagePath === right.imagePath
  );
}

function recheckDue(flow: GuidedFlow): boolean {
  return flow.cardReinserted && (flow.step === "bootOnce" || flow.step === "copy");
}

function nextStep(step: GuidedStep): GuidedStep | "done" {
  const index = GUIDED_STEPS.indexOf(step);
  return GUIDED_STEPS[index + 1] ?? "done";
}

function refuseActive(flow: GuidedFlow, step: GuidedStep): string | null {
  if (flow.stopped) return flow.status || "This setup has stopped.";
  if (flow.step !== step) return "This step is not the current one.";
  return null;
}

function refuseDisk(diskNumber: number): string | null {
  if (!Number.isInteger(diskNumber) || diskNumber <= 0) {
    return "Disk 0 and any disk without a number are never used.";
  }
  return null;
}

function refuseWord(command: CommandName, typed: string): string | null {
  const expected = WORD[command];
  if (!expected) return null;
  if (typed !== expected) {
    return `Type ${expected} for this step. The setup does not type it for you, and a word from another step does not count.`;
  }
  return null;
}

function refuseRoute(flow: GuidedFlow, route: "backup" | "blank"): string | null {
  if (route === "backup" && (!flow.backupDone || flow.backupRouteDisabled)) {
    return "The backup route is off because this card cannot be restored. The blank route still applies.";
  }
  return null;
}

function refuseBackupName(destination: string): string | null {
  const trimmed = destination.trim();
  if (trimmed.length === 0) return "Choose where to save the backup.";
  const folded = trimmed.replace(/\//g, "\\").toLowerCase();
  if (folded.includes("\\\\.\\physicaldrive")) {
    return "A backup file cannot be a physical disk path.";
  }
  const slash = Math.max(trimmed.lastIndexOf("/"), trimmed.lastIndexOf("\\"));
  const name = (slash >= 0 ? trimmed.slice(slash + 1) : trimmed).toLowerCase();
  if (!name.endsWith(".img.gz") || name.includes(".partial")) {
    return "Choose a .img.gz file. Partial backups are not a destination.";
  }
  return null;
}

function okCall(partial: Partial<FlowCall> & { command: CommandName }): Plan {
  return {
    ok: true,
    call: {
      command: partial.command,
      confirmation: partial.confirmation ?? "",
      route: partial.route ?? "",
      diskNumber: partial.diskNumber ?? 0,
      profileId: partial.profileId ?? "",
      volumeId: partial.volumeId ?? "",
      destination: partial.destination ?? "",
      imagePath: partial.imagePath ?? "",
    },
  };
}
