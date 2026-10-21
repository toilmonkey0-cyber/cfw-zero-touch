import { describe, expect, it } from "vitest";
import type { StockDetection } from "./detectDevice";
import {
  BOOT_ONCE_STEPS,
  GUIDED_STEPS,
  SKIP_WARNINGS,
  STEP_COMMAND,
  applyDetection,
  chooseDevice,
  confirmSuggestedDevice,
  finishBackup,
  failIssued,
  finishCopy,
  finishFlash,
  finishFullCheck,
  finishProbe,
  finishRestore,
  markCardReturned,
  planBackup,
  planCopy,
  planDetect,
  planFlash,
  planFullCheck,
  planProbe,
  planRestore,
  probeRouteFlag,
  skipStep,
  skipWarning,
  startGuidedFlow,
  type FlowCall,
  type GuidedFlow,
  type GuidedStep,
  type Plan,
} from "./guidedFlow";

const DISK = 2;
const BACKUP = "stock.img.gz";

const matched: StockDetection = {
  status: "matched",
  profileId: "r36s-clone-card",
  device: "R36S clone (board G80CA-MB V1.3)",
  screen: "",
  profileName: "R36S clone — games card",
  message: "",
};

const otherMatch: StockDetection = {
  ...matched,
  profileId: "darkos-rgb10x-os",
  device: "RGB10X",
  profileName: "dArkOS",
};

const noMatch: StockDetection = {
  status: "none",
  profileId: "",
  device: "",
  screen: "",
  profileName: "",
  message: "",
};

function take(plan: Plan): FlowCall {
  if (!plan.ok) {
    throw new Error(plan.reason);
  }
  return plan.call;
}

function backup(flow: GuidedFlow): GuidedFlow {
  const call = take(planBackup(flow, { diskNumber: DISK, destination: BACKUP }));
  return finishBackup(flow, call, { status: "complete", summary: "saved" });
}

function probe(flow: GuidedFlow, route: "backup" | "blank"): GuidedFlow {
  const call = take(
    planProbe(flow, { diskNumber: DISK, confirmation: "PROBE", route }),
  );
  return finishProbe(flow, call, {
    status: "finished",
    kind: "genuine",
    overwritten: true,
    summary: "Part of this card was overwritten. Restore your backup before you use this card.",
  });
}

function fullCheck(flow: GuidedFlow, route: "backup" | "blank"): GuidedFlow {
  const call = take(
    planFullCheck(flow, { diskNumber: DISK, confirmation: "FULL", route }),
  );
  return finishFullCheck(flow, call, {
    status: "finished",
    badCount: 0,
    overwritten: true,
    summary: "The full check finished. The card was overwritten.",
  });
}

function detectAndConfirm(flow: GuidedFlow): GuidedFlow {
  const call = take(planDetect(flow));
  const detected = applyDetection(flow, call, matched);
  expect(detected.confirmedProfileId).toBe("");
  expect(detected.suggestedProfileId).toBe(matched.profileId);
  expect(planFlash(detected, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
  return confirmSuggestedDevice(detected);
}

function flash(flow: GuidedFlow): GuidedFlow {
  const call = take(planFlash(flow, { diskNumber: DISK, confirmation: "FLASH" }));
  expect(call.profileId).toBe(flow.confirmedProfileId);
  expect(call.confirmation).toBe("FLASH");
  expect(call.command).toBe("flash_os");
  return finishFlash(flow, call, { ok: true, summary: "RESULT=OK" });
}

function recheck(flow: GuidedFlow, detection: StockDetection = matched): GuidedFlow {
  const returned = markCardReturned(flow);
  expect(planCopy(returned, { volumeId: "E" }).ok).toBe(false);
  const call = take(planDetect(returned));
  return applyDetection(returned, call, detection);
}

function copy(flow: GuidedFlow): GuidedFlow {
  const call = take(planCopy(flow, { volumeId: "E" }));
  expect(call.command).toBe("copy_roms");
  expect(call.profileId).toBe(flow.confirmedProfileId);
  expect(call.confirmation).toBe("");
  return finishCopy(flow, call, { ok: true, summary: "copied" });
}

function complete(flow: GuidedFlow, step: GuidedStep): GuidedFlow {
  if (step === "backup") return backup(flow);
  if (step === "probe") return probe(flow, flow.backupRouteDisabled ? "blank" : "backup");
  if (step === "fullCheck") return fullCheck(flow, flow.backupRouteDisabled ? "blank" : "backup");
  if (step === "detect") return detectAndConfirm(flow);
  if (step === "flash") return flash(flow);
  if (step === "bootOnce") return recheck(flow, noMatch);
  return copy(flow);
}

/** Runs every step, optionally skipping one. A skipped detect still needs a picked device. */
function drive(skip?: GuidedStep): GuidedFlow {
  let flow = startGuidedFlow();
  for (const step of GUIDED_STEPS) {
    expect(flow.stopped).toBe(false);
    expect(flow.step).toBe(step);
    if (skip === step) {
      const warning = skipWarning(flow);
      expect(warning).toBe(SKIP_WARNINGS[step]);
      flow = skipStep(flow);
      expect(flow.warnings).toContain(warning);
      if (step === "detect") {
        flow = chooseDevice(flow, "darkos-rgb10x-os");
      }
      if (step === "bootOnce") {
        flow = recheck(flow, noMatch);
      }
      continue;
    }
    flow = complete(flow, step);
  }
  return flow;
}

function reach(step: GuidedStep): GuidedFlow {
  let flow = startGuidedFlow();
  for (const current of GUIDED_STEPS) {
    if (current === step) return flow;
    flow = complete(flow, current);
  }
  return flow;
}

function commands(flow: GuidedFlow): string[] {
  return flow.calls.map((call) => call.command);
}

describe("guided flow happy path", () => {
  it("chains backup, probe, full check, detect, flash, boot-once, and copy", () => {
    const flow = drive();
    expect(flow.step).toBe("done");
    expect(flow.stopped).toBe(false);
    expect(flow.status).not.toContain("OVERWRITTEN");
    expect(commands(flow)).toEqual([
      "backup_stock_card",
      "probe_card",
      "full_check_card",
      "detect_stock_device",
      "flash_os",
      "detect_stock_device",
      "copy_roms",
    ]);
    expect(flow.calls.map((call) => call.confirmation)).toEqual([
      "",
      "PROBE",
      "FULL",
      "",
      "FLASH",
      "",
      "",
    ]);
    expect(flow.calls[1]?.route).toBe("backup");
    expect(flow.calls[2]?.route).toBe("backup");
    expect(commands(flow)).not.toContain("restore_backup");
    expect(flow.confirmedProfileId).toBe(matched.profileId);
    expect(BOOT_ONCE_STEPS.join(" ")).toMatch(/handheld/i);
    expect(BOOT_ONCE_STEPS.join(" ")).toMatch(/back/i);
  });

  it("keeps going after a genuine probe that overwrites part of the card", () => {
    const flow = probe(backup(startGuidedFlow()), "backup");
    expect(flow.stopped).toBe(false);
    expect(flow.step).toBe("fullCheck");
    expect(flow.status).not.toContain("OVERWRITTEN");
    expect(flow.note).toContain("overwritten");
  });
});

describe("guided flow skip paths", () => {
  it.each(GUIDED_STEPS)(
    "skipping %s warns in plain language and does not run that step",
    (step) => {
      const flow = drive(step);
      expect(flow.step).toBe("done");
      expect(flow.stopped).toBe(false);
      expect(flow.warnings).toContain(SKIP_WARNINGS[step]);
      const command = STEP_COMMAND[step];
      if (step === "detect") {
        const flashAt = commands(flow).indexOf("flash_os");
        const detectAt = commands(flow).indexOf("detect_stock_device");
        expect(detectAt).toBeGreaterThan(flashAt);
        expect(commands(flow).filter((name) => name === "detect_stock_device")).toHaveLength(1);
      } else if (command) {
        expect(commands(flow)).not.toContain(command);
      }
      expect(commands(flow)).not.toContain("restore_backup");
    },
  );

  it("skipping backup says the card cannot be restored and leaves only the blank route", () => {
    const flow = skipStep(startGuidedFlow());
    expect(flow.step).toBe("probe");
    expect(flow.backupRouteDisabled).toBe(true);
    expect(SKIP_WARNINGS.backup).toMatch(/cannot be restored/i);
    expect(flow.warnings[0]).toMatch(/cannot be restored/i);
    expect(probeRouteFlag(flow)).toMatch(/backup route is off/i);
    expect(probeRouteFlag(flow)).toMatch(/blank route/i);
    const backupRoute = planProbe(flow, {
      diskNumber: DISK,
      confirmation: "PROBE",
      route: "backup",
    });
    expect(backupRoute.ok).toBe(false);
    if (!backupRoute.ok) {
      expect(backupRoute.reason).toMatch(/blank route/i);
    }
    const blankRoute = planProbe(flow, {
      diskNumber: DISK,
      confirmation: "PROBE",
      route: "blank",
    });
    expect(blankRoute.ok).toBe(true);
    if (blankRoute.ok) {
      expect(blankRoute.call.route).toBe("blank");
      expect(blankRoute.call.confirmation).toBe("PROBE");
    }
    const finished = drive("backup");
    expect(finished.calls.find((call) => call.command === "probe_card")?.route).toBe("blank");
    expect(finished.calls.some((call) => call.route === "backup")).toBe(false);
  });
});

describe("guided flow stops", () => {
  it("stops on a cancelled probe and shows OVERWRITTEN without continuing", () => {
    const ready = reach("probe");
    const call = take(planProbe(ready, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }));
    const stopped = finishProbe(ready, call, {
      status: "cancelled",
      kind: "",
      overwritten: true,
      summary: "Data on the card was overwritten.",
    });
    expect(stopped.stopped).toBe(true);
    expect(stopped.step).toBe("probe");
    expect(stopped.status).toContain("OVERWRITTEN");
    expect(stopped.status).toContain("Data on the card was overwritten.");
    expect(planFullCheck(stopped, { diskNumber: DISK, confirmation: "FULL", route: "backup" }).ok).toBe(
      false,
    );
    expect(planFlash(stopped, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
    expect(planCopy(stopped, { volumeId: "E" }).ok).toBe(false);
    expect(commands(stopped)).toEqual(["backup_stock_card", "probe_card"]);
  });

  it("does not say OVERWRITTEN when the cancel says nothing was overwritten", () => {
    const ready = reach("probe");
    const call = take(planProbe(ready, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }));
    const stopped = finishProbe(ready, call, {
      status: "cancelled",
      kind: "",
      overwritten: false,
      summary: "Cancelled before the test pattern was written.",
    });
    expect(stopped.stopped).toBe(true);
    expect(stopped.status).not.toContain("OVERWRITTEN");
    expect(stopped.status).toMatch(/nothing on the card was overwritten/i);
    expect(planFullCheck(stopped, { diskNumber: DISK, confirmation: "FULL", route: "blank" }).ok).toBe(
      false,
    );
  });

  it("does not invent OVERWRITTEN when the result never said", () => {
    const ready = reach("probe");
    const call = take(planProbe(ready, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }));
    const stopped = finishProbe(ready, call, {
      status: "cancelled",
      kind: "",
      overwritten: null,
      summary: "The log ended early.",
    });
    expect(stopped.status).not.toContain("OVERWRITTEN");
    expect(stopped.status).toMatch(/did not say whether the card was overwritten/i);
    expect(planFlash(stopped, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
  });

  it("stops when the quick test finds a fake card", () => {
    const ready = reach("probe");
    const call = take(planProbe(ready, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }));
    const stopped = finishProbe(ready, call, {
      status: "finished",
      kind: "fake",
      overwritten: true,
      summary: "This card is not the size it claims.",
    });
    expect(stopped.stopped).toBe(true);
    expect(stopped.status).toContain("OVERWRITTEN");
    expect(planFullCheck(stopped, { diskNumber: DISK, confirmation: "FULL", route: "backup" }).ok).toBe(
      false,
    );
    expect(planFlash(stopped, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
  });

  it("stops when the full check finds bad spots", () => {
    const ready = reach("fullCheck");
    const call = take(
      planFullCheck(ready, { diskNumber: DISK, confirmation: "FULL", route: "backup" }),
    );
    const stopped = finishFullCheck(ready, call, {
      status: "finished",
      badCount: 2,
      overwritten: true,
      summary: "Two ranges did not read back.",
    });
    expect(stopped.stopped).toBe(true);
    expect(stopped.status).toContain("OVERWRITTEN");
    expect(planDetect(stopped).ok).toBe(false);
    expect(planFlash(stopped, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
  });

  it.each(["backup", "detect", "flash", "copy"] as const)(
    "a failure at %s stops the flow before the next write",
    (step) => {
      const ready = reach(step);
      let stopped: GuidedFlow;
      if (step === "backup") {
        const call = take(planBackup(ready, { diskNumber: DISK, destination: BACKUP }));
        stopped = finishBackup(ready, call, { status: "failed", summary: "the read failed" });
        expect(stopped.status).not.toContain("OVERWRITTEN");
        expect(stopped.status).toMatch(/nothing on the card was overwritten/i);
      } else if (step === "detect") {
        const call = take(planDetect(ready));
        const manual = applyDetection(ready, call, {
          ...matched,
          status: "manual",
          profileId: "",
          device: "",
          profileName: "",
        });
        expect(manual.stopped).toBe(false);
        expect(applyDetection(ready, { ...call, command: "backup_stock_card" }, matched)).toBe(ready);
        stopped = failIssued(ready, call, {
          outcome: "failed",
          overwritten: true,
          summary: "could not read the card",
        });
        expect(stopped.status).not.toContain("OVERWRITTEN");
        expect(stopped.status).toMatch(/nothing on the card was overwritten/i);
      } else if (step === "flash") {
        const call = take(planFlash(ready, { diskNumber: DISK, confirmation: "FLASH" }));
        stopped = finishFlash(ready, call, {
          ok: false,
          summary: "the flash did not finish",
          overwritten: null,
          outcome: "failed",
        });
        expect(stopped.status).not.toContain("OVERWRITTEN");
        expect(stopped.status).toMatch(/did not say whether the card was overwritten/i);
      } else {
        const call = take(planCopy(ready, { volumeId: "E" }));
        stopped = finishCopy(ready, call, {
          ok: false,
          summary: "the copy stopped",
          overwritten: null,
          outcome: "failed",
        });
        expect(stopped.status).not.toContain("OVERWRITTEN");
      }
      expect(stopped.stopped).toBe(true);
      expect(planProbe(stopped, { diskNumber: DISK, confirmation: "PROBE", route: "blank" }).ok).toBe(
        false,
      );
      expect(planFullCheck(stopped, { diskNumber: DISK, confirmation: "FULL", route: "blank" }).ok).toBe(
        false,
      );
      expect(planFlash(stopped, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
      expect(planCopy(stopped, { volumeId: "E" }).ok).toBe(false);
      expect(planBackup(stopped, { diskNumber: DISK, destination: BACKUP }).ok).toBe(false);
    },
  );
});

describe("device confirmation and boot-once", () => {
  it("pre-selects a match and waits for the user to confirm the device", () => {
    const ready = reach("detect");
    const call = take(planDetect(ready));
    const detected = applyDetection(ready, call, matched);
    expect(detected.step).toBe("detect");
    expect(detected.suggestedProfileId).toBe("r36s-clone-card");
    expect(detected.confirmedProfileId).toBe("");
    expect(detected.detectionMessage).toContain("R36S clone (board G80CA-MB V1.3)");
    expect(planFlash(detected, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
    expect(confirmSuggestedDevice(startGuidedFlow()).confirmedProfileId).toBe("");
    const confirmed = confirmSuggestedDevice(detected);
    expect(confirmed.confirmedProfileId).toBe("r36s-clone-card");
    expect(confirmed.step).toBe("flash");
    expect(commands(confirmed)).toEqual(["backup_stock_card", "probe_card", "full_check_card", "detect_stock_device"]);
  });

  it("lets the user confirm a handheld after skipping detection and flash", () => {
    const skipped = skipStep(skipStep(reach("detect")));
    expect(skipped.step).toBe("bootOnce");
    expect(skipped.confirmedProfileId).toBe("");
    const pickedEarly = chooseDevice(skipped, "darkos-rgb10x-os");
    expect(pickedEarly.step).toBe("bootOnce");
    expect(pickedEarly.confirmedProfileId).toBe("darkos-rgb10x-os");
    const checked = recheck(pickedEarly, noMatch);
    expect(planCopy(checked, { volumeId: "E" }).ok).toBe(true);

    const stillWaiting = recheck(skipped, noMatch);
    expect(stillWaiting.step).toBe("copy");
    expect(planCopy(stillWaiting, { volumeId: "E" }).ok).toBe(false);
    const pickedLate = chooseDevice(stillWaiting, "darkos-rgb10x-os");
    expect(pickedLate.step).toBe("copy");
    expect(pickedLate.confirmedProfileId).toBe("darkos-rgb10x-os");
    expect(planCopy(pickedLate, { volumeId: "E" }).ok).toBe(true);
    expect(chooseDevice(checked, "r36s-clone-card")).toBe(checked);
  });

  it("uses the handheld the user picks when they decline the suggestion", () => {
    const ready = reach("detect");
    const call = take(planDetect(ready));
    const detected = applyDetection(ready, call, matched);
    const chosen = chooseDevice(detected, "darkos-rgb10x-os");
    expect(chosen.confirmedProfileId).toBe("darkos-rgb10x-os");
    expect(chosen.suggestedProfileId).toBe("r36s-clone-card");
    expect(chosen.step).toBe("flash");
    const flashing = take(planFlash(chosen, { diskNumber: DISK, confirmation: "FLASH" }));
    expect(flashing.profileId).toBe("darkos-rgb10x-os");
  });

  it("copies only after the card is back and detect has run again", () => {
    const booting = reach("bootOnce");
    expect(planCopy(booting, { volumeId: "E" }).ok).toBe(false);
    expect(markCardReturned(reach("flash")).cardReinserted).toBe(false);
    const returned = markCardReturned(booting);
    expect(returned.cardReinserted).toBe(true);
    expect(returned.identityRechecked).toBe(false);
    expect(returned.step).toBe("bootOnce");
    expect(planCopy(returned, { volumeId: "E" }).ok).toBe(false);
    if (!planCopy(returned, { volumeId: "E" }).ok) {
      expect(planCopy(returned, { volumeId: "E" }).ok).toBe(false);
    }
    const refused = planCopy(returned, { volumeId: "E" });
    expect(refused.ok).toBe(false);
    if (!refused.ok) {
      expect(refused.reason).toMatch(/recognized/i);
    }
    const call = take(planDetect(returned));
    const same = applyDetection(returned, call, matched);
    expect(same.step).toBe("copy");
    expect(same.identityRechecked).toBe(true);
    expect(planCopy(same, { volumeId: "E" }).ok).toBe(true);

    const unknown = applyDetection(returned, call, noMatch);
    expect(unknown.stopped).toBe(false);
    expect(unknown.step).toBe("copy");
    expect(planCopy(unknown, { volumeId: "E" }).ok).toBe(true);

    const mismatch = applyDetection(returned, call, otherMatch);
    expect(mismatch.stopped).toBe(true);
    expect(mismatch.status).toMatch(/different handheld/i);
    expect(planCopy(mismatch, { volumeId: "E" }).ok).toBe(false);
    expect(planFlash(mismatch, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
  });
});

describe("confirmations stay separate", () => {
  it("refuses a word from another step, a blank word, and disk 0", () => {
    const probing = reach("probe");
    expect(planProbe(probing, { diskNumber: DISK, confirmation: "", route: "backup" }).ok).toBe(false);
    expect(planProbe(probing, { diskNumber: DISK, confirmation: "FLASH", route: "backup" }).ok).toBe(
      false,
    );
    expect(planProbe(probing, { diskNumber: 0, confirmation: "PROBE", route: "backup" }).ok).toBe(false);
    const checking = reach("fullCheck");
    expect(planFullCheck(checking, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }).ok).toBe(
      false,
    );
    expect(planFullCheck(checking, { diskNumber: 0, confirmation: "FULL", route: "backup" }).ok).toBe(
      false,
    );
    const flashing = detectAndConfirm(reach("detect"));
    expect(planFlash(flashing, { diskNumber: DISK, confirmation: "PROBE" }).ok).toBe(false);
    expect(planFlash(flashing, { diskNumber: DISK, confirmation: "FULL" }).ok).toBe(false);
    expect(planFlash(flashing, { diskNumber: 0, confirmation: "FLASH" }).ok).toBe(false);
    const planned = take(planProbe(probing, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }));
    const forged = finishProbe(probing, { ...planned, confirmation: "FLASH" }, {
      status: "finished",
      kind: "genuine",
      overwritten: true,
      summary: "should not apply",
    });
    expect(forged).toBe(probing);
    expect(planBackup(startGuidedFlow(), { diskNumber: 0, destination: BACKUP }).ok).toBe(false);
  });

  it("restores only as its own confirmed action and does not resume the flow", () => {
    const ready = reach("probe");
    const call = take(planProbe(ready, { diskNumber: DISK, confirmation: "PROBE", route: "backup" }));
    const stopped = finishProbe(ready, call, {
      status: "cancelled",
      kind: "",
      overwritten: true,
      summary: "Data on the card was overwritten.",
    });
    expect(commands(stopped)).not.toContain("restore_backup");
    expect(planRestore(startGuidedFlow(), { diskNumber: DISK, confirmation: "FLASH", imagePath: BACKUP }).ok).toBe(
      false,
    );
    expect(
      planRestore(stopped, { diskNumber: DISK, confirmation: "PROBE", imagePath: BACKUP }).ok,
    ).toBe(false);
    expect(
      planRestore(stopped, {
        diskNumber: DISK,
        confirmation: "FLASH",
        imagePath: "stock.img.gz.partial",
      }).ok,
    ).toBe(false);
    const restore = take(
      planRestore(stopped, { diskNumber: DISK, confirmation: "FLASH", imagePath: BACKUP }),
    );
    expect(restore.command).toBe("restore_backup");
    expect(restore.confirmation).toBe("FLASH");
    const after = finishRestore(stopped, restore, { ok: true, summary: "restore matched" });
    expect(after.stopped).toBe(true);
    expect(commands(after)).toContain("restore_backup");
    expect(planFlash(after, { diskNumber: DISK, confirmation: "FLASH" }).ok).toBe(false);
    expect(planCopy(after, { volumeId: "E" }).ok).toBe(false);
    expect(planFullCheck(after, { diskNumber: DISK, confirmation: "FULL", route: "blank" }).ok).toBe(
      false,
    );
  });
});
