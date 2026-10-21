import { useEffect, useRef, useState, type ReactNode } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";
import { stockImagePath } from "./backupPath";
import type { StockDetection } from "./detectDevice";
import { formatBytes } from "./formatBytes";
import { fullConfirmPrompt, fullProgressLabel } from "./fullCheckPath";
import {
  BOOT_ONCE_STEPS,
  GUIDED_STEPS,
  STEP_TITLES,
  applyDetection,
  chooseDevice,
  confirmSuggestedDevice,
  failIssued,
  finishBackup,
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
  repeatIdentityCheck,
  skipStep,
  skipWarning,
  startGuidedFlow,
  type DiskIdentity,
  type GuidedFlow,
} from "./guidedFlow";
import { probeConfirmPrompt } from "./probePath";
import {
  isFinishedBackupName,
  restoreConfirmPrompt,
  restoreDiskModel,
} from "./restorePath";

type Profile = {
  id: string;
  name: string;
  cfw: string;
  romSchema: {
    layout: string;
    volumeLabel?: string;
    systems: { id: string; folder: string; extensions: string[] }[];
  };
  image?: { url: string } | null;
};

type Disk = { number: number; name: string; sizeBytes: number };

type Volume = {
  id: string;
  letter: string;
  label: string;
  decision: "ready" | "needs_format" | "rejected";
};

type Firstboot = { state: "safe" | "armed" | "unknown"; reason: string };

type PlanView = { warning: string | null; copyCount: number; skipCount: number };

type BackupReport = { status: string; image: string; partial: string };

type ProbeReport = {
  status: string;
  kind: string;
  overwritten: boolean;
  summary: string;
};

type FullReport = ProbeReport & { badCount: number };

type CopyReport = { copied: number; skipped: number; bytesCopied: number };

type RestoreReport = { status: string; detail: string };

/**
 * Beginner setup. Every disk write is an existing command, and each one keeps
 * its own typed confirmation. This component does not format or flash on its own.
 */
export function GuidedWizard({ onExit }: { onExit: () => void }) {
  const [flow, setFlow] = useState<GuidedFlow>(startGuidedFlow);
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [disks, setDisks] = useState<Disk[]>([]);
  const [busy, setBusy] = useState(false);
  const [confirmation, setConfirmation] = useState("");
  const [route, setRoute] = useState<"backup" | "blank">("blank");
  const [backupDest, setBackupDest] = useState("");
  const [restoreImage, setRestoreImage] = useState("");
  const [notice, setNotice] = useState("");
  const [library, setLibrary] = useState("");
  const [included, setIncluded] = useState<string[]>([]);
  const [planView, setPlanView] = useState<PlanView | null>(null);
  const [volume, setVolume] = useState<Volume | null>(null);
  const [progress, setProgress] = useState("");
  const [identities, setIdentities] = useState<Record<number, DiskIdentity>>({});
  const [identityChecked, setIdentityChecked] = useState<Record<number, boolean>>({});
  const held = useRef(false);

  useEffect(() => {
    let live = true;
    invoke<Profile[]>("list_profiles")
      .then((found) => {
        if (live) setProfiles(found);
      })
      .catch((cause: unknown) => {
        if (live) setNotice(String(cause));
      });
    return () => {
      live = false;
    };
  }, []);

  useEffect(() => {
    setConfirmation("");
  }, [flow.step, flow.stopped]);

  useEffect(() => {
    if (flow.backupRouteDisabled) setRoute("blank");
    else if (flow.backupDone) setRoute("backup");
  }, [flow.backupDone, flow.backupRouteDisabled]);

  useEffect(() => {
    const profile = profiles.find((item) => item.id === flow.confirmedProfileId);
    setIncluded(profile ? profile.romSchema.systems.map((system) => system.id) : []);
    setPlanView(null);
  }, [flow.confirmedProfileId, profiles]);

  useEffect(() => {
    const wantsReturnList = flow.step === "bootOnce" || flow.step === "copy";
    const wantsFlashList = flow.stopped || flow.step === "backup" || flow.step === "flash";
    const wantsProbeList = flow.step === "probe" || flow.step === "fullCheck" || wantsReturnList;
    if (!wantsFlashList && !wantsProbeList) return;
    let live = true;
    const command = wantsProbeList ? "list_probe_disks" : "list_flash_disks";
    invoke<Disk[]>(command)
      .then(async (found) => {
        if (!live) return;
        setDisks(found);
        const next: Record<number, DiskIdentity> = {};
        const checked: Record<number, boolean> = {};
        await Promise.all(
          found.map(async (disk) => {
            checked[disk.number] = true;
            try {
              next[disk.number] = await invoke<DiskIdentity>("read_disk_identity", {
                diskNumber: disk.number,
              });
            } catch {
              // A missing serial leaves this disk disabled.
            }
          }),
        );
        if (live) {
          setIdentities(next);
          setIdentityChecked(checked);
        }
      })
      .catch((cause: unknown) => {
        if (live) setNotice(String(cause));
      });
    return () => {
      live = false;
    };
  }, [flow.step, flow.stopped]);

  useEffect(() => {
    const unlisten = listen<{ bytesRead: number; bytesTotal: number }>("backup-progress", (event) => {
      setProgress(
        `Read ${formatBytes(event.payload.bytesRead)} of ${formatBytes(event.payload.bytesTotal)}`,
      );
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<string>("probe-progress", (event) => setProgress(event.payload));
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<{ phase: string; percent: number; mbps: number; etaSeconds: number }>(
      "fullcheck-progress",
      (event) => setProgress(fullProgressLabel(event.payload)),
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<string>("flash-progress", (event) => setProgress(event.payload));
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<{ index: number; total: number; relativeDest: string; done: boolean }>(
      "copy-progress",
      (event) => {
        setProgress(
          event.payload.done
            ? "Copy finished."
            : `Copying ${event.payload.index + 1} of ${event.payload.total}: ${event.payload.relativeDest}`,
        );
      },
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  const profile = profiles.find((item) => item.id === flow.confirmedProfileId) ?? null;

  function knownIdentity(disk: Disk): DiskIdentity {
    return identities[disk.number] ?? { diskNumber: disk.number, serial: "", sizeBytes: 0 };
  }

  async function freshIdentity(diskNumber: number): Promise<DiskIdentity | null> {
    try {
      const identity = await invoke<DiskIdentity>("read_disk_identity", { diskNumber });
      setIdentities((current) => ({ ...current, [diskNumber]: identity }));
      return identity;
    } catch (cause) {
      setNotice(String(cause));
      return null;
    }
  }

  function startOver() {
    setFlow(startGuidedFlow());
    setConfirmation("");
    setNotice("");
    setProgress("");
    setVolume(null);
    setPlanView(null);
    setLibrary("");
    setRestoreImage("");
    setBackupDest("");
    setRoute("blank");
  }

  async function chooseBackupDestination() {
    const selected = await save({
      title: "Save stock-card backup",
      defaultPath: "stock-card.img.gz",
      filters: [{ name: "Compressed disk image", extensions: ["gz"] }],
    });
    if (typeof selected === "string" && selected.trim().length > 0) {
      setBackupDest(stockImagePath(selected));
    }
  }

  async function chooseRestoreImage() {
    const selected = await open({
      title: "Choose a stock-card backup",
      filters: [{ name: "Compressed disk image", extensions: ["gz"] }],
      multiple: false,
    });
    if (typeof selected === "string" && selected.trim().length > 0) {
      setRestoreImage(selected.trim());
    }
  }

  async function runBackup(disk: Disk) {
    const snapshot = flow;
    const identity = await freshIdentity(disk.number);
    if (!identity) return;
    const planned = planBackup(snapshot, { ...identity, destination: backupDest });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    setProgress("Reading the card. This does not erase it.");
    try {
      const report = await invoke<BackupReport>("backup_stock_card", {
        diskNumber: identity.diskNumber,
        displayedBytes: identity.sizeBytes,
        destination: planned.call.destination,
      });
      const status = report.status === "complete" || report.status === "cancelled" ? report.status : "failed";
      const summary =
        status === "complete"
          ? `Saved ${report.image}.`
          : `Cancelled. Partial file: ${report.partial}`;
      setFlow(finishBackup(snapshot, planned.call, { status, summary }));
    } catch (cause) {
      setFlow(finishBackup(snapshot, planned.call, { status: "failed", summary: String(cause) }));
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function runProbe(disk: Disk) {
    const snapshot = flow;
    const identity = await freshIdentity(disk.number);
    if (!identity) return;
    const planned = planProbe(snapshot, {
      ...identity,
      confirmation,
      route,
    });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    setProgress("Checking the card. Windows may ask for approval before anything is written.");
    try {
      const report = await invoke<ProbeReport>("probe_card", {
        diskNumber: identity.diskNumber,
        displayedBytes: identity.sizeBytes,
        displayedModel: restoreDiskModel(disk.name),
        confirmation: planned.call.confirmation,
      });
      const status =
        report.status === "finished" || report.status === "cancelled" ? report.status : "failed";
      setFlow(
        finishProbe(snapshot, planned.call, {
          status,
          kind: report.kind,
          overwritten: report.overwritten,
          summary: report.summary,
        }),
      );
    } catch (cause) {
      setFlow(
        failIssued(snapshot, planned.call, {
          outcome: "failed",
          overwritten: null,
          summary: String(cause),
        }),
      );
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function runFullCheck(disk: Disk) {
    const snapshot = flow;
    const identity = await freshIdentity(disk.number);
    if (!identity) return;
    const planned = planFullCheck(snapshot, {
      ...identity,
      confirmation,
      route,
    });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    setProgress(
      "Checking the whole card. Windows may ask for approval before anything is written. This test erases the card.",
    );
    try {
      const report = await invoke<FullReport>("full_check_card", {
        diskNumber: identity.diskNumber,
        displayedBytes: identity.sizeBytes,
        displayedModel: restoreDiskModel(disk.name),
        confirmation: planned.call.confirmation,
      });
      const status =
        report.status === "finished" || report.status === "cancelled" ? report.status : "failed";
      setFlow(
        finishFullCheck(snapshot, planned.call, {
          status,
          badCount: report.badCount,
          overwritten: report.overwritten,
          summary: report.summary,
        }),
      );
    } catch (cause) {
      setFlow(
        failIssued(snapshot, planned.call, {
          outcome: "failed",
          overwritten: null,
          summary: String(cause),
        }),
      );
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function runDetect() {
    const snapshot = flow;
    const planned = planDetect(snapshot);
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    try {
      const detection = await invoke<StockDetection>("detect_stock_device");
      const next = applyDetection(snapshot, planned.call, detection);
      setFlow(next);
      if (next.step === "copy" && !next.stopped) await findReadyVolume(next);
    } catch (cause) {
      setFlow(
        failIssued(snapshot, planned.call, {
          outcome: "failed",
          overwritten: null,
          summary: String(cause),
        }),
      );
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function runFlash(disk: Disk) {
    const snapshot = flow;
    const identity = await freshIdentity(disk.number);
    if (!identity) return;
    const planned = planFlash(snapshot, { ...identity, confirmation });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    setProgress("Checking the OS image…");
    try {
      const log = await invoke<string>("flash_os", {
        profileId: planned.call.profileId,
        diskNumber: identity.diskNumber,
        confirmation: planned.call.confirmation,
      });
      setFlow(finishFlash(snapshot, planned.call, { ok: true, summary: log }));
    } catch (cause) {
      setFlow(
        finishFlash(snapshot, planned.call, {
          ok: false,
          summary: String(cause),
          overwritten: null,
          outcome: "failed",
        }),
      );
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function cardIsBack(disk: Disk) {
    let snapshot = flow;
    if (snapshot.step === "copy" && snapshot.identityRechecked) {
      snapshot = repeatIdentityCheck(snapshot);
    }
    const returned = markCardReturned(snapshot);
    if (!returned.cardReinserted) {
      setNotice("Put the card back in this computer after the handheld has booted once.");
      return;
    }
    const planned = planDetect(returned);
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    setFlow(returned);
    setVolume(null);
    setPlanView(null);
    const unrecognized: StockDetection = {
      status: "none",
      profileId: "",
      device: "",
      screen: "",
      profileName: "",
      message: "",
    };
    try {
      let identity: DiskIdentity;
      try {
        identity = await invoke<DiskIdentity>("read_disk_identity", { diskNumber: disk.number });
      } catch (cause) {
        setNotice(String(cause));
        setFlow(
          applyDetection(returned, planned.call, unrecognized, {
            diskNumber: disk.number,
            serial: "",
            sizeBytes: 0,
          }),
        );
        return;
      }
      let detection = unrecognized;
      try {
        detection = await invoke<StockDetection>("detect_stock_device");
      } catch (cause) {
        setNotice(String(cause));
      }
      const next = applyDetection(returned, planned.call, detection, identity);
      setFlow(next);
      if (next.step === "copy" && !next.stopped) await findReadyVolume(next);
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function findReadyVolume(current: GuidedFlow) {
    setVolume(null);
    const chosen = profiles.find((item) => item.id === current.confirmedProfileId);
    if (!chosen) {
      setNotice("Choose the handheld before copying.");
      return;
    }
    try {
      const found = await invoke<Volume[]>("list_volumes", { profileId: chosen.id });
      const label = chosen.romSchema.volumeLabel ?? "";
      const ready = found.find((item) => {
        if (item.decision !== "ready") return false;
        if (label.length === 0) return true;
        return item.label.toLowerCase() === label.toLowerCase();
      });
      if (!ready) {
        setNotice(
          `${label || "The games partition"} is not on a removable card yet. Boot the handheld once, shut it down, then put the card back.`,
        );
        return;
      }
      const state = await invoke<Firstboot>("firstboot_state", {
        profileId: chosen.id,
        volumeId: ready.id,
      });
      if (state.state !== "safe") {
        setNotice(state.reason);
        return;
      }
      setVolume(ready);
      setNotice("");
    } catch (cause) {
      setNotice(String(cause));
    }
  }

  async function chooseLibrary() {
    const selected = await open({ directory: true, multiple: false, title: "ROM library" });
    if (typeof selected === "string") {
      setLibrary(selected);
      setPlanView(null);
    }
  }

  async function previewCopy() {
    if (!profile || !volume || !library) return;
    const planned = planCopy(flow, { volumeId: volume.id });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    try {
      const next = await invoke<PlanView>("plan_roms", {
        profileId: planned.call.profileId,
        volumeId: planned.call.volumeId,
        library,
        include: included,
      });
      setPlanView(next);
    } catch (cause) {
      setNotice(String(cause));
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function runCopy() {
    if (!volume || !library) return;
    const snapshot = flow;
    const planned = planCopy(snapshot, { volumeId: volume.id });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    setProgress("Starting copy…");
    try {
      const state = await invoke<Firstboot>("firstboot_state", {
        profileId: planned.call.profileId,
        volumeId: planned.call.volumeId,
      });
      if (state.state !== "safe") {
        setNotice(state.reason);
        setVolume(null);
        return;
      }
      const report = await invoke<CopyReport>("copy_roms", {
        profileId: planned.call.profileId,
        volumeId: planned.call.volumeId,
        library,
        include: included,
        dryRun: false,
      });
      setFlow(
        finishCopy(snapshot, planned.call, {
          ok: true,
          summary: `Copied ${report.copied} files (${formatBytes(report.bytesCopied)}). Skipped ${report.skipped} unchanged files.`,
        }),
      );
    } catch (cause) {
      setFlow(
        finishCopy(snapshot, planned.call, {
          ok: false,
          summary: String(cause),
          overwritten: null,
          outcome: "failed",
        }),
      );
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  async function runRestore(disk: Disk) {
    const snapshot = flow;
    const identity = await freshIdentity(disk.number);
    if (!identity) return;
    const planned = planRestore(snapshot, {
      ...identity,
      confirmation,
      imagePath: restoreImage,
    });
    if (!planned.ok) {
      setNotice(planned.reason);
      return;
    }
    if (held.current) return;
    held.current = true;
    setBusy(true);
    setNotice("");
    try {
      const report = await invoke<RestoreReport>("restore_backup", {
        diskNumber: identity.diskNumber,
        displayedBytes: identity.sizeBytes,
        displayedModel: restoreDiskModel(disk.name),
        confirmation: planned.call.confirmation,
        imagePath: planned.call.imagePath,
      });
      if (report.status === "pass") {
        setFlow(finishRestore(snapshot, planned.call, { ok: true, summary: report.detail }));
      } else {
        setFlow(
          finishRestore(snapshot, planned.call, {
            ok: false,
            summary: report.detail,
            overwritten: null,
            outcome: report.status === "cancelled" ? "cancelled" : "failed",
          }),
        );
      }
    } catch (cause) {
      setFlow(
        finishRestore(snapshot, planned.call, {
          ok: false,
          summary: String(cause),
          overwritten: null,
          outcome: "failed",
        }),
      );
    } finally {
      held.current = false;
      setBusy(false);
    }
  }

  const cancelCommand = flow.stopped
    ? "cancel_restore"
    : flow.step === "backup"
      ? "cancel_stock_backup"
      : flow.step === "probe"
        ? "cancel_probe"
        : flow.step === "fullCheck"
          ? "cancel_full_check"
          : "";

  const confirmedName = profile?.name ?? "";

  return (
    <section>
      <h2>Guided setup</h2>
      <p>
        Back up the card, test it, confirm the handheld, flash, boot once, then copy games.
        Each erase or test asks for its own word. This setup does not type those words, and one
        word does not approve the next step.
      </p>
      <ol className="guided-steps">
        {GUIDED_STEPS.map((step) => (
          <li key={step} className={flow.step === step ? "current" : undefined}>
            {STEP_TITLES[step]}
          </li>
        ))}
      </ol>
      {flow.warnings.map((warning) => (
        <p key={warning} className="warning">
          {warning}
        </p>
      ))}
      {flow.stopped ? <p className="error">{flow.status}</p> : null}
      {flow.note ? <p className="path">{flow.note}</p> : null}
      {notice ? <p className="error">{notice}</p> : null}
      {progress ? <p>{progress}</p> : null}

      {flow.step === "backup" && !flow.stopped ? (
        <div>
          <h3>Back up the card</h3>
          <p>
            Reads the whole removable card into a compressed .img.gz. The card is opened
            read-only. Fixed disks and disk 0 are not listed.
          </p>
          <div className="row">
            <button disabled={busy} onClick={() => void chooseBackupDestination()}>
              Choose where to save
            </button>
          </div>
          {backupDest ? <p className="path">{backupDest}</p> : <p>Choose a .img.gz file first.</p>}
          <DiskList
            disks={disks}
            empty="No USB disk detected."
            action={(disk) => (
              <div>
                <IdentityNote disk={disk} identities={identities} identityChecked={identityChecked} />
                <button
                  disabled={busy || !planBackup(flow, { ...knownIdentity(disk), destination: backupDest }).ok}
                  onClick={() => void runBackup(disk)}
                >
                  Back up disk {disk.number}
                </button>
              </div>
            )}
          />
        </div>
      ) : null}

      {flow.step === "probe" && !flow.stopped ? (
        <TestStep
          title="Quick card test"
          intro="Writes a short test pattern. A finished test overwrites part of the card. The card check still refuses a card that is not blank and has no backup."
          flag={probeRouteFlag(flow)}
          route={route}
          backupRouteDisabled={flow.backupRouteDisabled}
          onRoute={setRoute}
          confirmation={confirmation}
          onConfirmation={setConfirmation}
          disks={disks}
          busy={busy}
          prompt={(disk) =>
            probeConfirmPrompt({
              number: disk.number,
              name: restoreDiskModel(disk.name),
              sizeLabel: formatBytes(disk.sizeBytes),
            })
          }
          allowed={(disk) =>
            planProbe(flow, { ...knownIdentity(disk), confirmation, route }).ok
          }
          run={runProbe}
          verb="Test"
          note={(disk) => (
            <IdentityNote disk={disk} identities={identities} identityChecked={identityChecked} />
          )}
        />
      ) : null}

      {flow.step === "fullCheck" && !flow.stopped ? (
        <TestStep
          title="Full card test"
          intro="Optional. Writes a pattern across the whole card and reads it back. A finished test erases the card. You can skip this and keep the quick test result."
          flag={probeRouteFlag(flow)}
          route={route}
          backupRouteDisabled={flow.backupRouteDisabled}
          onRoute={setRoute}
          confirmation={confirmation}
          onConfirmation={setConfirmation}
          disks={disks}
          busy={busy}
          prompt={(disk) =>
            fullConfirmPrompt({
              number: disk.number,
              name: restoreDiskModel(disk.name),
              sizeLabel: formatBytes(disk.sizeBytes),
            })
          }
          allowed={(disk) =>
            planFullCheck(flow, { ...knownIdentity(disk), confirmation, route }).ok
          }
          run={runFullCheck}
          verb="Erase and test"
          note={(disk) => (
            <IdentityNote disk={disk} identities={identities} identityChecked={identityChecked} />
          )}
        />
      ) : null}

      {flow.step === "detect" && !flow.stopped ? (
        <div>
          <h3>Check which handheld this is</h3>
          <p>
            Detection only highlights a layout. Nothing is chosen until you press Use this device
            or pick a different layout.
          </p>
          <button disabled={busy} onClick={() => void runDetect()}>
            Check the card
          </button>
          {flow.detectionMessage ? <p>{flow.detectionMessage}</p> : null}
          {flow.suggestedProfileId ? (
            <div className="row">
              <button disabled={busy} onClick={() => setFlow(confirmSuggestedDevice(flow))}>
                Use this device
              </button>
            </div>
          ) : null}
          <ProfileList
            profiles={profiles}
            suggestedId={flow.suggestedProfileId}
            busy={busy}
            onPick={(id) => setFlow(chooseDevice(flow, id))}
          />
        </div>
      ) : null}

      {flow.step === "flash" && !flow.stopped ? (
        <div>
          <h3>Flash the system</h3>
          <p>
            {confirmedName
              ? `Confirmed handheld: ${confirmedName}.`
              : "Choose the handheld. A highlighted suggestion is not confirmed yet."}
          </p>
          <ProfileList
            profiles={profiles}
            suggestedId={flow.suggestedProfileId}
            busy={busy}
            onPick={(id) => setFlow(chooseDevice(flow, id))}
          />
          {profile && !profile.image ? (
            <p>
              This layout has no OS image. Skip flashing, or choose a layout that includes one.
              Games are copied only after you boot the card once and put it back.
            </p>
          ) : null}
          {profile?.image ? (
            <DiskList
              disks={disks}
              empty="No USB disk detected."
              action={(disk) => (
                <div>
                  <IdentityNote disk={disk} identities={identities} identityChecked={identityChecked} />
                  <label>
                    Type FLASH to erase disk {disk.number} ({formatBytes(disk.sizeBytes)})
                    <input
                      value={confirmation}
                      onChange={(event) => setConfirmation(event.target.value)}
                      autoComplete="off"
                    />
                  </label>
                  <button
                    disabled={busy || !planFlash(flow, { ...knownIdentity(disk), confirmation }).ok}
                    onClick={() => void runFlash(disk)}
                  >
                    Flash OS
                  </button>
                </div>
              )}
            />
          ) : null}
        </div>
      ) : null}

      {flow.step === "bootOnce" && !flow.stopped ? (
        <div>
          <h3>Boot the handheld once</h3>
          <ol>
            {BOOT_ONCE_STEPS.map((line) => (
              <li key={line}>{line}</li>
            ))}
          </ol>
          {flow.confirmedProfileId.length === 0 ? (
            <div>
              <p>Choose the handheld. Copy stays off until you do.</p>
              <ProfileList
                profiles={profiles}
                suggestedId={flow.suggestedProfileId}
                busy={busy}
                onPick={(id) => setFlow(chooseDevice(flow, id))}
              />
            </div>
          ) : null}
          <p>Choose the card you took out. Copy stays off until that same card is recognized.</p>
          <ReturnedDisks
            disks={disks}
            identities={identities}
            identityChecked={identityChecked}
            busy={busy}
            label="This card is back"
            onPick={(disk) => void cardIsBack(disk)}
          />
        </div>
      ) : null}

      {flow.step === "copy" && !flow.stopped ? (
        <div>
          <h3>Copy your games</h3>
          <p>
            Copy stays off until the same card is back in this computer and that card has been
            recognized again.
          </p>
          <ReturnedDisks
            disks={disks}
            identities={identities}
            identityChecked={identityChecked}
            busy={busy}
            label={flow.identityRechecked ? "Check this card again" : "This card is back"}
            onPick={(disk) => void cardIsBack(disk)}
          />
          {flow.confirmedProfileId.length === 0 ? (
            <div>
              <p>Choose the handheld before copying.</p>
              <ProfileList
                profiles={profiles}
                suggestedId={flow.suggestedProfileId}
                busy={busy}
                onPick={(id) => {
                  const next = chooseDevice(flow, id);
                  setFlow(next);
                  if (next.identityRechecked && next.step === "copy") void findReadyVolume(next);
                }}
              />
            </div>
          ) : null}
          {volume && profile ? (
            <div>
              <p>
                Copying onto {volume.letter}: ({volume.label || "unlabeled"}). Your ROMs stay on
                this PC. This app never downloads games.
              </p>
              <button disabled={busy} onClick={() => void chooseLibrary()}>
                Choose library folder
              </button>
              {library ? <p className="path">{library}</p> : null}
              <ul className="systems">
                {profile.romSchema.systems.map((system) => (
                  <li key={system.id}>
                    <label>
                      <input
                        type="checkbox"
                        checked={included.includes(system.id)}
                        onChange={() => {
                          setPlanView(null);
                          setIncluded((current) =>
                            current.includes(system.id)
                              ? current.filter((id) => id !== system.id)
                              : [...current, system.id],
                          );
                        }}
                      />
                      {system.folder}
                    </label>
                  </li>
                ))}
              </ul>
              <div className="row">
                <button disabled={busy || !library} onClick={() => void previewCopy()}>
                  Preview
                </button>
                <button
                  disabled={
                    busy ||
                    !planView ||
                    planView.copyCount + planView.skipCount === 0 ||
                    !planCopy(flow, { volumeId: volume.id }).ok
                  }
                  onClick={() => void runCopy()}
                >
                  Copy onto card
                </button>
              </div>
              {planView?.warning ? <p className="error">{planView.warning}</p> : null}
              {planView && !planView.warning ? (
                <p>
                  {planView.copyCount} to copy, {planView.skipCount} already on the card.
                </p>
              ) : null}
            </div>
          ) : null}
        </div>
      ) : null}

      {flow.step === "done" ? (
        <div>
          <h3>Safe to eject</h3>
          <p>Eject the card from this computer and put it in the handheld.</p>
        </div>
      ) : null}

      {flow.stopped ? (
        <div>
          <h3>Restore a backup</h3>
          <p>
            This setup has stopped, so it will not flash or copy. Restoring is a separate step.
            Type FLASH for the disk you select. A word from an earlier step does not count.
          </p>
          {flow.backupRouteDisabled ? (
            <p className="warning">
              This setup has no backup, so the card cannot be restored unless you already saved one.
            </p>
          ) : null}
          <button disabled={busy} onClick={() => void chooseRestoreImage()}>
            Choose backup
          </button>
          {restoreImage ? <p className="path">{restoreImage}</p> : null}
          {restoreImage && !isFinishedBackupName(restoreImage) ? (
            <p className="error">Choose a finished .img.gz. Partial backups are refused.</p>
          ) : null}
          <DiskList
            disks={disks}
            empty="No USB disk detected."
            action={(disk) => (
              <div>
                <IdentityNote disk={disk} identities={identities} identityChecked={identityChecked} />
                <label>
                  {restoreConfirmPrompt({
                    number: disk.number,
                    name: restoreDiskModel(disk.name),
                    sizeLabel: formatBytes(disk.sizeBytes),
                  })}
                  <input
                    value={confirmation}
                    onChange={(event) => setConfirmation(event.target.value)}
                    autoComplete="off"
                  />
                </label>
                <button
                  disabled={
                    busy ||
                    !planRestore(flow, {
                      ...knownIdentity(disk),
                      confirmation,
                      imagePath: restoreImage,
                    }).ok
                  }
                  onClick={() => void runRestore(disk)}
                >
                  Restore onto disk {disk.number}
                </button>
              </div>
            )}
          />
        </div>
      ) : null}

      {busy && cancelCommand ? (
        <button onClick={() => void invoke(cancelCommand)}>Cancel this step</button>
      ) : null}

      {!flow.stopped && flow.step !== "done" ? (
        <div>
          <p className="warning">{skipWarning(flow)}</p>
          <button
            disabled={busy}
            onClick={() => {
              setNotice("");
              setFlow(skipStep(flow));
            }}
          >
            Skip this step
          </button>
        </div>
      ) : null}

      <div className="row">
        <button disabled={busy} onClick={startOver}>
          Start over
        </button>
        <button disabled={busy} onClick={onExit}>
          Back to the start screen
        </button>
      </div>
    </section>
  );
}

function IdentityNote({
  disk,
  identities,
  identityChecked,
}: {
  disk: Disk;
  identities: Record<number, DiskIdentity>;
  identityChecked: Record<number, boolean>;
}) {
  if (!identityChecked[disk.number] || identities[disk.number]) return null;
  return <p className="warning">This card did not report a serial, so it cannot be recognized.</p>;
}

function ReturnedDisks({
  disks,
  identities,
  identityChecked,
  busy,
  label,
  onPick,
}: {
  disks: Disk[];
  identities: Record<number, DiskIdentity>;
  identityChecked: Record<number, boolean>;
  busy: boolean;
  label: string;
  onPick: (disk: Disk) => void;
}) {
  return (
    <DiskList
      disks={disks}
      empty="No USB or SD card detected."
      action={(disk) => (
        <div>
          <IdentityNote disk={disk} identities={identities} identityChecked={identityChecked} />
          <button disabled={busy || !identities[disk.number]} onClick={() => onPick(disk)}>
            {label}
          </button>
        </div>
      )}
    />
  );
}

function DiskList({
  disks,
  empty,
  action,
}: {
  disks: Disk[];
  empty: string;
  action: (disk: Disk) => ReactNode;
}) {
  if (disks.length === 0) return <p>{empty}</p>;
  return (
    <ul className="volumes">
      {disks.map((disk) => (
        <li key={disk.number}>
          <div>
            <strong>
              Disk {disk.number}: {restoreDiskModel(disk.name)}
            </strong>
            <span>{formatBytes(disk.sizeBytes)}</span>
          </div>
          {action(disk)}
        </li>
      ))}
    </ul>
  );
}

function ProfileList({
  profiles,
  suggestedId,
  busy,
  onPick,
}: {
  profiles: Profile[];
  suggestedId: string;
  busy: boolean;
  onPick: (id: string) => void;
}) {
  if (profiles.length === 0) return <p>No card layouts found.</p>;
  return (
    <div className="cards">
      {profiles.map((item) => (
        <button
          key={item.id}
          className={item.id === suggestedId ? "card suggested" : "card"}
          disabled={busy}
          onClick={() => onPick(item.id)}
        >
          <strong>{item.name}</strong>
          <span>{item.cfw}</span>
        </button>
      ))}
    </div>
  );
}

function TestStep({
  title,
  intro,
  flag,
  route,
  backupRouteDisabled,
  onRoute,
  confirmation,
  onConfirmation,
  disks,
  busy,
  prompt,
  allowed,
  run,
  verb,
  note,
}: {
  title: string;
  intro: string;
  flag: string;
  route: "backup" | "blank";
  backupRouteDisabled: boolean;
  onRoute: (route: "backup" | "blank") => void;
  confirmation: string;
  onConfirmation: (value: string) => void;
  disks: Disk[];
  busy: boolean;
  prompt: (disk: Disk) => string;
  allowed: (disk: Disk) => boolean;
  run: (disk: Disk) => Promise<void>;
  verb: string;
  note?: (disk: Disk) => ReactNode;
}) {
  return (
    <div>
      <h3>{title}</h3>
      <p>{intro}</p>
      <p className={backupRouteDisabled ? "warning" : undefined}>{flag}</p>
      <div className="row">
        <label>
          <input
            type="radio"
            name="probe-route"
            checked={route === "backup"}
            disabled={backupRouteDisabled || busy}
            onChange={() => onRoute("backup")}
          />
          Backup route
        </label>
        <label>
          <input
            type="radio"
            name="probe-route"
            checked={route === "blank"}
            disabled={busy}
            onChange={() => onRoute("blank")}
          />
          Blank route
        </label>
      </div>
      <DiskList
        disks={disks}
        empty="No USB or SD card detected."
        action={(disk) => (
          <div>
            {note ? note(disk) : null}
            <label>
              {prompt(disk)}
              <input
                value={confirmation}
                onChange={(event) => onConfirmation(event.target.value)}
                autoComplete="off"
              />
            </label>
            <button disabled={busy || !allowed(disk)} onClick={() => void run(disk)}>
              {verb} disk {disk.number}
            </button>
          </div>
        )}
      />
    </div>
  );
}
