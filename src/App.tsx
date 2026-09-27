import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open } from "@tauri-apps/plugin-dialog";
import { openUrl, revealItemInDir } from "@tauri-apps/plugin-opener";
import "./App.css";

type SystemFolder = { id: string; folder: string; extensions: string[] };

type Profile = {
  id: string;
  version?: number;
  name: string;
  cfw: string;
  storageModes: string[];
  romSchema: {
    layout: string;
    volumeLabel?: string;
    biosFolder?: string;
    systems: SystemFolder[];
  };
  image?: { url: string; sha256: string; compressed?: string; notes?: string } | null;
};

type FlashDisk = { number: number; name: string; sizeBytes: number };

type VolumeView = {
  id: string;
  letter: string;
  label: string;
  fileSystem: string;
  totalBytes: number;
  isEmpty: boolean;
  decision: "ready" | "needs_format" | "rejected";
  relabel: boolean;
  reason: string;
};

type PlanView = {
  files: { relativeDest: string; bytes: number; action: string }[];
  warning: string | null;
  copyCount: number;
  skipCount: number;
  duplicates: {
    systemId: string;
    resident: string;
    incoming: { relativeDest: string; size: number; similarity: number }[];
  }[];
  duplicateSkipped: number;
};

type CopyReport = { copied: number; skipped: number; bytesCopied: number };

type ProfileChange = { id: string; name: string; localVersion: number; remoteVersion: number };

type FeedCheck = {
  status: "up_to_date" | "updates" | "error";
  reason: string;
  updates: ProfileChange[];
  additions: string[];
  appUpdate: { version: string; releaseUrl: string } | null;
};

type ApplyReport = { applied: number; skipped: number };

type StageReport = {
  stagingPath: string;
  stagedSystems: string[];
  rawCopiedSystems: string[];
  stagedFiles: number;
};

type FirstbootState = { state: "safe" | "armed" | "unknown"; reason: string };

type DoctorReport = {
  heading: string;
  steps: string[];
  engineText: string | null;
  suggestedProfileId: string | null;
};

type Step = "profile" | "card" | "library" | "flash" | "bootOnce" | "done";

type SortMode = "none" | "smart" | "dat";

type NeedleArtifactStatus = {
  id: string;
  role: string;
  fileName: string;
  size: number;
  present: boolean;
  verified: boolean;
  path: string | null;
};

type VariantDecision =
  | { kind: "Keep" }
  | { kind: "SkipRegion"; winner: string }
  | { kind: "SkipDuplicate"; kept: string };

type Classification = {
  routes: {
    relative: string;
    size: number;
    systemId: string;
    tier: string;
    variant: VariantDecision;
  }[];
  needsReview: {
    reviewId: string;
    relative: string;
    size: number;
    nearestStem: string | null;
    nearestSystem: string | null;
    similarity: number | null;
  }[];
  groups: {
    systemId: string;
    stem: string;
    kept: number;
    skippedVariants: number;
    skippedDuplicates: number;
  }[];
  indexStats: { titles: number; rebuilt: boolean; dim: number };
  coldStart: boolean;
};

type Resolution = { systemId: string | null; size: number };

function formatBytes(bytes: number): string {
  if (bytes < 0) return "unknown size";
  if (bytes < 1000) return `${bytes} B`;
  const mb = bytes / 1_000_000;
  if (mb < 1000) return mb < 10 ? `${mb.toFixed(1)} MB` : `${Math.round(mb)} MB`;
  return `${(bytes / 1_000_000_000).toFixed(1)} GB`;
}

export default function App() {
  const [step, setStep] = useState<Step>("profile");
  const [profiles, setProfiles] = useState<Profile[]>([]);
  const [profile, setProfile] = useState<Profile | null>(null);
  const [volumes, setVolumes] = useState<VolumeView[]>([]);
  const [volume, setVolume] = useState<VolumeView | null>(null);
  const [confirmation, setConfirmation] = useState("");
  const [folders, setFolders] = useState<string[]>([]);
  const [library, setLibrary] = useState("");
  const [included, setIncluded] = useState<string[]>([]);
  const [plan, setPlan] = useState<PlanView | null>(null);
  const [progress, setProgress] = useState("");
  const [report, setReport] = useState<CopyReport | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const [flashDisks, setFlashDisks] = useState<FlashDisk[]>([]);
  const [flashLog, setFlashLog] = useState("");
  const [keepExistingFolders, setKeepExistingFolders] = useState(false);
  const [feedCheck, setFeedCheck] = useState<FeedCheck | null>(null);
  const [checkingFeed, setCheckingFeed] = useState(false);
  const [appVersion, setAppVersion] = useState("");
  const [applyingFeed, setApplyingFeed] = useState(false);
  const [feedMessage, setFeedMessage] = useState("");
  const [diagPath, setDiagPath] = useState("");
  const [datFolder, setDatFolder] = useState("");
  const [regions, setRegions] = useState<string[]>(["USA"]);
  const [single, setSingle] = useState(true);
  const [staging, setStaging] = useState(false);
  const [stageSummary, setStageSummary] = useState("");
  const [stagedLibrary, setStagedLibrary] = useState("");
  const [sortMode, setSortMode] = useState<SortMode>("none");
  const [needleStatus, setNeedleStatus] = useState<NeedleArtifactStatus[] | null>(null);
  const [engineBusy, setEngineBusy] = useState(false);
  const [engineMessage, setEngineMessage] = useState("");
  const [classification, setClassification] = useState<Classification | null>(null);
  const [resolutions, setResolutions] = useState<Record<string, Resolution>>({});
  const [dedupeKeep, setDedupeKeep] = useState<string[]>([]);
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [doctorBusy, setDoctorBusy] = useState(false);
  const [doctorEngine, setDoctorEngine] = useState(false);

  useEffect(() => {
    invoke<Profile[]>("list_profiles")
      .then(setProfiles)
      .catch((cause: unknown) => setError(String(cause)));
  }, []);

  useEffect(() => {
    void checkFeed(true);
  }, []);

  useEffect(() => {
    invoke<string>("app_version")
      .then(setAppVersion)
      .catch(() => setAppVersion(""));
  }, []);

  useEffect(() => {
    invoke<string>("diagnostics_path")
      .then(setDiagPath)
      .catch(() => setDiagPath(""));
  }, []);

  useEffect(() => {
    refreshNeedleStatus();
  }, []);

  useEffect(() => {
    const unlisten = listen<string>("needle-progress", (event) => {
      setEngineMessage(event.payload);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  async function refreshNeedleStatus() {
    try {
      setNeedleStatus(await invoke<NeedleArtifactStatus[]>("needle_status"));
    } catch {
      setNeedleStatus(null);
    }
  }

  async function installEngine() {
    setEngineBusy(true);
    setEngineMessage("Downloading the smart-sort engine…");
    try {
      await invoke<string>("needle_acquire", { artifactId: "weights" });
      setEngineMessage("Smart-sort engine ready.");
      await refreshNeedleStatus();
    } catch (cause) {
      setEngineMessage(`Engine download failed: ${String(cause)}`);
    } finally {
      setEngineBusy(false);
    }
  }

  function engineReady(): boolean {
    const weights = needleStatus?.find((row) => row.id === "weights");
    return Boolean(weights?.present && weights?.verified);
  }

  function serveReady(): boolean {
    const engine = needleStatus?.find((row) => row.id === "serve-engine");
    return (
      engineReady() && Boolean(engine?.present && engine?.verified)
    );
  }

  async function explainCard(item: VolumeView) {
    if (!profile) return;
    setDoctorBusy(true);
    setError("");
    try {
      const report = await invoke<DoctorReport>("diagnose_card", {
        profileId: profile.id,
        volumeId: item.id,
        withEngine: doctorEngine && serveReady(),
      });
      setDoctor(report);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setDoctorBusy(false);
    }
  }

  async function checkFeed(silent = false) {
    if (!silent) setBusy(true);
    setCheckingFeed(true);
    try {
      setFeedCheck(await invoke<FeedCheck>("check_profile_feed"));
      setFeedMessage("");
    } catch (cause) {
      setFeedCheck({
        status: "error",
        reason: String(cause),
        updates: [],
        additions: [],
        appUpdate: null,
      });
    } finally {
      setCheckingFeed(false);
      if (!silent) setBusy(false);
    }
  }

  async function applyFeed() {
    setApplyingFeed(true);
    setError("");
    try {
      const report = await invoke<ApplyReport>("apply_profile_feed");
      setFeedMessage(`Updated ${report.applied} profile${report.applied === 1 ? "" : "s"}.`);
      setProfiles(await invoke<Profile[]>("list_profiles"));
      await checkFeed(true);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setApplyingFeed(false);
    }
  }

  useEffect(() => {
    const unlisten = listen<string>("flash-progress", (event) => {
      setFlashLog(event.payload);
    });
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  useEffect(() => {
    const unlisten = listen<{ relativeDest: string; index: number; total: number; done: boolean }>(
      "copy-progress",
      (event) => {
        if (event.payload.done) {
          setProgress("Copy finished.");
          return;
        }
        setProgress(
          `Copying ${event.payload.index + 1} of ${event.payload.total}: ${event.payload.relativeDest}`,
        );
      },
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, []);

  async function refreshVolumes(nextProfile: Profile) {
    setBusy(true);
    setError("");
    try {
      const found = await invoke<VolumeView[]>("list_volumes", { profileId: nextProfile.id });
      setVolumes(found);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function chooseProfile(next: Profile) {
    setProfile(next);
    setIncluded(next.romSchema.systems.map((system) => system.id));
    setVolume(null);
    setDoctor(null);
    setDoctorEngine(false);
    setConfirmation("");
    setFlashLog("");
    if (next.image) {
      setStep("flash");
      await refreshFlashDisks();
      return;
    }
    setStep("card");
    await refreshVolumes(next);
  }

  async function refreshFlashDisks() {
    setBusy(true);
    setError("");
    try {
      setFlashDisks(await invoke<FlashDisk[]>("list_flash_disks"));
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function flashOs(disk: FlashDisk) {
    if (!profile) return;
    setBusy(true);
    setError("");
    setFlashLog("Checking the OS image…");
    try {
      const log = await invoke<string>("flash_os", {
        profileId: profile.id,
        diskNumber: disk.number,
        confirmation,
      });
      setFlashLog(log);
      setStep(profile.romSchema.layout === "arkos_easyroms_root" ? "bootOnce" : "done");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function findExpandedCard() {
    if (!profile) return;
    setBusy(true);
    setError("");
    try {
      const found = await invoke<VolumeView[]>("list_volumes", { profileId: profile.id });
      const label = profile.romSchema.volumeLabel ?? "EASYROMS";
      const ready = found.find(
        (item) => item.label.toLowerCase() === label.toLowerCase() && item.decision === "ready",
      );
      if (!ready) {
        setError(
          `${label} is not on a removable card yet. Boot the handheld once, shut it down, then insert the card.`,
        );
        return;
      }
      const state = await invoke<FirstbootState>("firstboot_state", {
        profileId: profile.id,
        volumeId: ready.id,
      });
      if (state.state === "armed") {
        setError(
          `${state.reason} This card has not finished first boot: the handheld would format ${label} and erase anything copied now. Boot it until the game menu appears, shut down from the menu, then put the card back.`,
        );
        return;
      }
      if (state.state === "unknown") {
        setError(
          `${state.reason} Check that the card's small BOOT partition is visible in Windows, reinsert the card, and try again.`,
        );
        return;
      }
      setVolume(ready);
      setFolders([]);
      setKeepExistingFolders(true);
      setStep("library");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function prepare(selected: VolumeView) {
    if (!profile) return;
    setBusy(true);
    setError("");
    try {
      const updated = await invoke<VolumeView>("prepare_card", {
        profileId: profile.id,
        volumeId: selected.id,
        confirmation: selected.decision === "needs_format" ? confirmation : "",
        displayedBytes: selected.totalBytes,
      });
      const seeded = await invoke<{ folders: string[] }>("seed_card", {
        profileId: profile.id,
        volumeId: selected.id,
      });
      setVolume(updated);
      setFolders(seeded.folders);
      setStep("library");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function chooseLibrary() {
    setError("");
    const selected = await open({ directory: true, multiple: false, title: "ROM library" });
    if (typeof selected === "string") {
      setLibrary(selected);
      resetPlan();
    }
  }

  function resetPlan() {
    setPlan(null);
    setStagedLibrary("");
    setStageSummary("");
    setClassification(null);
    setResolutions({});
    setDedupeKeep([]);
  }

  function setSortChoice(mode: SortMode) {
    resetPlan();
    setSortMode(mode);
    setEngineMessage("");
    if (mode === "smart") {
      void refreshNeedleStatus();
    }
  }

  async function chooseDatFolder() {
    const selected = await open({ directory: true, multiple: false, title: "DAT folder" });
    if (typeof selected === "string") {
      setDatFolder(selected);
      resetPlan();
    }
  }

  function toggleRegion(region: string) {
    resetPlan();
    setRegions((current) =>
      current.includes(region) ? current.filter((item) => item !== region) : [...current, region],
    );
  }

  async function preview() {
    if (!profile || !volume || !library) return;
    if (sortMode === "smart") {
      await previewSmart();
      return;
    }
    setBusy(true);
    setError("");
    try {
      let source = library;
      if (sortMode === "dat" && datFolder) {
        setStaging(true);
        setProgress("Staging a sorted copy with igir…");
        const report = await invoke<StageReport>("stage_library", {
          profileId: profile.id,
          library,
          datFolder,
          regions,
          single,
          include: included,
        });
        source = report.stagingPath;
        setStagedLibrary(report.stagingPath);
        const sorted = report.stagedSystems.length;
        const raw = report.rawCopiedSystems.length;
        setStageSummary(
          `Staged ${report.stagedFiles} files: ${sorted} system${sorted === 1 ? "" : "s"} sorted with the DAT` +
            (raw > 0 ? `, ${raw} copied as-is (no DAT pattern).` : "."),
        );
        setStaging(false);
      }
      const next = await invoke<PlanView>("plan_roms", {
        profileId: profile.id,
        volumeId: volume.id,
        library: source,
        include: included,
      });
      setPlan(next);
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
      setStaging(false);
    }
  }

  async function previewSmart() {
    if (!profile || !volume || !library) return;
    if (!engineReady()) {
      setError(
        "The smart-sort engine is not installed. Use Download below the Smart sort option (about 35 MB, checksum-verified), or choose another sort mode.",
      );
      return;
    }
    setBusy(true);
    setError("");
    setProgress("Reading the library…");
    try {
      const result = await invoke<Classification>("classify_library", {
        profileId: profile.id,
        library,
        include: included,
        regions,
      });
      setClassification(result);
      // Review choices default to the suggestion (skip when there is none).
      const defaults: Record<string, Resolution> = {};
      for (const row of result.needsReview) {
        defaults[row.reviewId] = {
          systemId: row.nearestSystem,
          size: row.size,
        };
      }
      setResolutions(defaults);
      setProgress("");
      if (result.needsReview.length === 0) {
        await planWithSmart();
      }
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  async function planWithSmart() {
    if (!profile || !volume || !library) return;
    setBusy(true);
    setError("");
    try {
      const next = await invoke<PlanView>("plan_roms", {
        profileId: profile.id,
        volumeId: volume.id,
        library,
        include: included,
        smart: { regions, resolutions },
        dedupe: { keep: dedupeKeep },
      });
      setPlan(next);
      // Grouped files default to skip; the preview keeps the user's
      // per-group choices in dedupeKeep.
      setDedupeKeep((current) =>
        current.filter((dest) =>
          next.duplicates.some((group) =>
            group.incoming.some((item) => item.relativeDest === dest),
          ),
        ),
      );
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  function toggleDedupeKeep(dest: string) {
    setDedupeKeep((current) =>
      current.includes(dest) ? current.filter((item) => item !== dest) : [...current, dest],
    );
  }

  function setResolution(reviewId: string, systemId: string | null) {
    const row = classification?.needsReview.find((item) => item.reviewId === reviewId);
    if (!row) return;
    setResolutions((current) => ({
      ...current,
      [reviewId]: { systemId, size: row.size },
    }));
  }

  function acceptAllSuggestions() {
    if (!classification) return;
    const defaults: Record<string, Resolution> = {};
    for (const row of classification.needsReview) {
      defaults[row.reviewId] = { systemId: row.nearestSystem, size: row.size };
    }
    setResolutions(defaults);
  }

  async function copyRoms() {
    if (!profile || !volume || !library) return;
    setBusy(true);
    setError("");
    setProgress("Starting copy…");
    try {
      const next = await invoke<CopyReport>("copy_roms", {
        profileId: profile.id,
        volumeId: volume.id,
        library: stagedLibrary || library,
        include: included,
        dryRun: false,
        ...(sortMode === "smart"
          ? { smart: { regions, resolutions }, dedupe: { keep: dedupeKeep } }
          : {}),
      });
      setReport(next);
      setStep("done");
    } catch (cause) {
      setError(String(cause));
    } finally {
      setBusy(false);
    }
  }

  function toggleSystem(id: string) {
    resetPlan();
    setIncluded((current) =>
      current.includes(id) ? current.filter((item) => item !== id) : [...current, id],
    );
  }

  return (
    <main>
      <header>
        <p className="eyebrow">ROMs card and OS flash</p>
        <h1>CFW Card Studio</h1>
        <p className="legal">
          Your ROMs and BIOS stay on this PC. This app never downloads games. Formatting erases the
          removable card you select.
        </p>
        {appVersion ? <p className="path">Card Studio {appVersion}</p> : null}
        {feedCheck?.appUpdate ? (
          <p>
            Card Studio {feedCheck.appUpdate.version} is available.
            <button onClick={() => void openUrl(feedCheck.appUpdate?.releaseUrl ?? "")}>
              Open the releases page
            </button>
          </p>
        ) : null}
        {diagPath ? (
          <p>
            Diagnostics: {diagPath}
            <button onClick={() => void revealItemInDir(diagPath)}>
              Show diagnostics log in folder
            </button>
          </p>
        ) : null}
      </header>

      {error ? <p className="error">{error}</p> : null}

      {step === "profile" ? (
        <section>
          <h2>Choose a card layout</h2>
          {feedCheck || checkingFeed ? (
            <div className="feed">
              {checkingFeed ? <p>Checking for profile updates…</p> : null}
              {feedCheck?.status === "up_to_date" ? <p>Profiles are up to date.</p> : null}
              {feedCheck?.status === "updates" ? (
                <p>
                  {feedCheck.updates.length + feedCheck.additions.length} profile update
                  {feedCheck.updates.length + feedCheck.additions.length === 1 ? "" : "s"}{" "}
                  available.
                </p>
              ) : null}
              {feedCheck?.status === "error" ? (
                <p className="error">Could not check for updates: {feedCheck.reason}</p>
              ) : null}
              {feedMessage ? <p>{feedMessage}</p> : null}
              <div className="row">
                <button disabled={busy || applyingFeed || checkingFeed} onClick={() => void checkFeed()}>
                  Check again
                </button>
                {feedCheck?.status === "updates" ? (
                  <button disabled={busy || applyingFeed} onClick={() => void applyFeed()}>
                    Update profiles
                  </button>
                ) : null}
              </div>
            </div>
          ) : null}
          {profiles.length === 0 ? <p>No ROMs-card profiles found.</p> : null}
          <div className="cards">
            {profiles.map((item) => (
              <button key={item.id} className="card" onClick={() => void chooseProfile(item)}>
                <strong>{item.name}</strong>
                <span>
                  {item.image
                    ? `${item.cfw} · writes an OS image · checksum checked`
                    : `${item.cfw} · ${item.romSchema.systems.length} systems · label ${item.romSchema.volumeLabel ?? "any"}`}
                </span>
              </button>
            ))}
          </div>
        </section>
      ) : null}

      {step === "flash" && profile ? (
        <section>
          <h2>Flash the OS card</h2>
          <p>
            {profile.name}. The OS image is downloaded and checked against its published checksum.
            This erases the USB disk you select. The internal drive is not listed. Windows asks for
            approval before the write starts.
          </p>
          <p>
            If a flash stops partway, run it again from this screen. The card only becomes usable
            after a write that matches the image when it is read back.
          </p>
          {profile.image?.notes ? <p>{profile.image.notes}</p> : null}
          {profile.romSchema.layout === "arkos_easyroms_root" ? (
            <button disabled={busy} onClick={() => void findExpandedCard()}>
              The card already booted
            </button>
          ) : null}
          <button disabled={busy} onClick={() => void refreshFlashDisks()}>
            Refresh disks
          </button>
          {flashDisks.length === 0 ? <p>No USB disk detected.</p> : null}
          <ul className="volumes">
            {flashDisks.map((disk) => (
              <li key={disk.number}>
                <div>
                  <strong>
                    Disk {disk.number}: {disk.name}
                  </strong>
                  <span>{formatBytes(disk.sizeBytes)}</span>
                </div>
                <label>
                  Type FLASH to erase disk {disk.number} ({formatBytes(disk.sizeBytes)})
                  <input
                    value={confirmation}
                    onChange={(event) => setConfirmation(event.target.value)}
                    autoComplete="off"
                  />
                </label>
                <button
                  disabled={busy || confirmation !== "FLASH"}
                  onClick={() => void flashOs(disk)}
                >
                  Flash OS
                </button>
              </li>
            ))}
          </ul>
          {flashLog ? <p className="path">{flashLog}</p> : null}
        </section>
      ) : null}

      {step === "card" && profile ? (
        <section>
          <h2>Prepare the ROMs card</h2>
          <p>
            {profile.name}. Insert a removable SD card. Fixed disks are hidden on purpose.
          </p>
          <button disabled={busy} onClick={() => void refreshVolumes(profile)}>
            Refresh drives
          </button>
          {volumes.length === 0 ? (
            <p>No removable drive detected. Insert the card, then refresh.</p>
          ) : (
            <ul className="volumes">
              {volumes.map((item) => (
                <li key={item.id}>
                  <div>
                    <strong>
                      {item.letter}: {item.label || "unlabeled"}
                    </strong>
                    <span>
                      {item.fileSystem || "unknown"} · {formatBytes(item.totalBytes)} ·{" "}
                      {item.isEmpty ? "empty" : "has files"}
                    </span>
                    {item.reason ? <span className="reason">{item.reason}</span> : null}
                    {item.relabel ? <span>The volume label will be updated. Files stay put.</span> : null}
                  </div>
                  <div className="row">
                    <button disabled={busy || doctorBusy} onClick={() => void explainCard(item)}>
                      {doctorBusy ? "Explaining…" : "Explain this card"}
                    </button>
                  </div>
                  {item.decision === "needs_format" ? (
                    <label>
                      Type FORMAT to erase {item.letter}: ({formatBytes(item.totalBytes)})
                      <input
                        value={confirmation}
                        onChange={(event) => setConfirmation(event.target.value)}
                        autoComplete="off"
                      />
                    </label>
                  ) : null}
                  <button
                    disabled={
                      busy ||
                      item.decision === "rejected" ||
                      (item.decision === "needs_format" && confirmation !== "FORMAT")
                    }
                    onClick={() => void prepare(item)}
                  >
                    {item.decision === "needs_format" ? "Erase and prepare" : "Use this card"}
                  </button>
                </li>
              ))}
            </ul>
          )}
          {doctor ? (
            <div className="doctor">
              <h3>What is on this card</h3>
              <p>
                <strong>{doctor.heading}</strong>
              </p>
              <ol>
                {doctor.steps.map((line) => (
                  <li key={line.slice(0, 64)}>{line}</li>
                ))}
              </ol>
              {doctor.engineText ? <p>{doctor.engineText}</p> : null}
              {doctor.suggestedProfileId ? (
                <p className="path">Suggested layout: {doctor.suggestedProfileId}</p>
              ) : null}
              <div className="row">
                <label>
                  <input
                    type="checkbox"
                    checked={doctorEngine}
                    onChange={() => setDoctorEngine(!doctorEngine)}
                  />
                  Use the local engine paraphrase when installed
                </label>
              </div>
              {!serveReady() ? (
                <p className="path">
                  Engine paraphrase needs the smart-sort weights plus the serve engine
                  (both checksum-verified downloads).
                </p>
              ) : null}
              <div className="row">
                <button disabled={busy || doctorBusy} onClick={() => setDoctor(null)}>
                  Close explanation
                </button>
              </div>
            </div>
          ) : null}
        </section>
      ) : null}

      {step === "library" && profile && volume ? (
        <section>
          <h2>Copy your library</h2>
          {keepExistingFolders ? (
            <p>
              Copying into the existing folders on {volume.letter}: ({volume.label}). Stock folders
              are left in place.
            </p>
          ) : (
            <p>
              Created on {volume.letter}: {folders.join(", ")}
            </p>
          )}
          <p>Library folders should be named like the system folders ({profile.romSchema.systems.map((system) => system.folder).join(", ")}).</p>
          <button onClick={() => void chooseLibrary()}>Choose library folder</button>
          {library ? <p className="path">{library}</p> : null}
          <div className="sort">
            <p>
              Sorting (optional). Smart sort needs no DAT: it routes files by folder, by unique
              file extension, and by title similarity against your organized folders. A DAT
              (a catalog you download yourself, e.g. from No-Intro's datomatic) trims to one game
              per title, filters regions, and verifies checksums.
            </p>
            <div className="row modes">
              {(
                [
                  ["none", "No sorting (copy as-is)"],
                  ["smart", "Smart sort (no DAT needed)"],
                  ["dat", "Sort with a DAT (igir)"],
                ] as [SortMode, string][]
              ).map(([mode, label]) => (
                <label key={mode} className={sortMode === mode ? "mode active" : "mode"}>
                  <input
                    type="radio"
                    name="sortMode"
                    checked={sortMode === mode}
                    onChange={() => setSortChoice(mode)}
                  />
                  {label}
                </label>
              ))}
            </div>
            {sortMode === "smart" ? (
              <div className="engine">
                {engineReady() ? (
                  <p className="chip ok">Smart-sort engine ready ({formatBytes(needleStatus?.find((row) => row.id === "weights")?.size ?? 35_000_000)}).</p>
                ) : (
                  <div className="row">
                    <p className="chip">
                      Engine not installed. Download about 35 MB, checksum-verified, to this PC
                      only.
                    </p>
                    <button disabled={busy || engineBusy} onClick={() => void installEngine()}>
                      {engineBusy ? "Downloading…" : "Download the smart-sort engine"}
                    </button>
                  </div>
                )}
                {engineMessage ? <p className="path">{engineMessage}</p> : null}
                {classification?.coldStart ? (
                  <p className="error">
                    Too few organized titles to auto-route ({classification.indexStats.titles}
                    {" "}indexed). Sort files into system folders first, or use a DAT.
                  </p>
                ) : null}
              </div>
            ) : null}
            {sortMode === "dat" ? (
              <div>
                <button onClick={() => void chooseDatFolder()}>Choose DAT folder</button>
                {datFolder ? <p className="path">{datFolder}</p> : null}
              </div>
            ) : null}
            {sortMode !== "none" ? (
              <div className="row">
                {["USA", "EUR", "JPN"].map((region) => (
                  <label key={region}>
                    <input
                      type="checkbox"
                      checked={regions.includes(region)}
                      onChange={() => toggleRegion(region)}
                    />
                    {region}
                  </label>
                ))}
                {sortMode === "dat" ? (
                  <label>
                    <input
                      type="checkbox"
                      checked={single}
                      disabled={!datFolder}
                      onChange={() => {
                        resetPlan();
                        setSingle(!single);
                      }}
                    />
                    One game per title (1G1R)
                  </label>
                ) : null}
              </div>
            ) : null}
            {stageSummary ? <p>{stageSummary}</p> : null}
          </div>
          {classification && sortMode === "smart" && !plan && classification.needsReview.length > 0 ? (
            <div className="review">
              <h3>
                Review {classification.needsReview.length} file
                {classification.needsReview.length === 1 ? "" : "s"}
              </h3>
              <p>
                Suggestions come from title similarity to your organized folders and are usually
                right — accept them all or adjust per file.
              </p>
              <div className="row">
                <button onClick={acceptAllSuggestions}>Accept all suggestions</button>
                <button disabled={busy} onClick={() => void planWithSmart()}>
                  Continue
                </button>
              </div>
              <table>
                <thead>
                  <tr>
                    <th>File</th>
                    <th>Nearest title</th>
                    <th>Match</th>
                    <th>Route to</th>
                  </tr>
                </thead>
                <tbody>
                  {classification.needsReview.map((row) => (
                    <tr key={row.reviewId}>
                      <td className="path">{row.relative}</td>
                      <td>{row.nearestStem ?? "—"}</td>
                      <td>
                        {row.similarity != null ? `${Math.round(row.similarity * 100)}%` : "—"}
                      </td>
                      <td>
                        <select
                          value={resolutions[row.reviewId]?.systemId ?? ""}
                          onChange={(event) =>
                            setResolution(
                              row.reviewId,
                              event.target.value === "" ? null : event.target.value,
                            )
                          }
                        >
                          <option value="">Skip this file</option>
                          {included.map((systemId) => (
                            <option key={systemId} value={systemId}>
                              {systemId}
                            </option>
                          ))}
                        </select>
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          ) : null}
          {classification && sortMode === "smart" && plan ? (
            <div className="variants">
              {classification.routes.length > classification.routes.filter((row) => row.variant.kind === "Keep").length ? (
                <p>
                  {classification.routes.filter((row) => row.variant.kind === "SkipRegion").length}{" "}
                  other-region variant
                  {classification.routes.filter((row) => row.variant.kind === "SkipRegion").length === 1 ? "" : "s"} and{" "}
                  {classification.routes.filter((row) => row.variant.kind === "SkipDuplicate").length} duplicate
                  {classification.routes.filter((row) => row.variant.kind === "SkipDuplicate").length === 1 ? "" : "s"}{" "}
                  skipped (kept your {regions.join(", ")} preference).
                </p>
              ) : null}
              <ul>
                {classification.groups
                  .filter((group) => group.skippedVariants + group.skippedDuplicates > 0)
                  .slice(0, 8)
                  .map((group) => (
                    <li key={`${group.systemId}/${group.stem}`}>
                      skip (variant) — {group.stem} ({group.systemId}): kept {group.kept}, skipped{" "}
                      {group.skippedVariants + group.skippedDuplicates}
                    </li>
                  ))}
              </ul>
            </div>
          ) : null}
          <ul className="systems">
            {profile.romSchema.systems.map((system) => (
              <li key={system.id}>
                <label>
                  <input
                    type="checkbox"
                    checked={included.includes(system.id)}
                    onChange={() => toggleSystem(system.id)}
                  />
                  {system.folder}
                  <span>{system.extensions.join(" ")}</span>
                </label>
              </li>
            ))}
          </ul>
          <div className="row">
            <button disabled={busy || !library} onClick={() => void preview()}>
              {staging ? "Staging…" : "Preview"}
            </button>
            <button disabled={busy || !plan || plan.copyCount + plan.skipCount === 0} onClick={() => void copyRoms()}>
              Copy onto card
            </button>
          </div>
          {plan?.warning ? <p className="error">{plan.warning}</p> : null}
          {plan && !plan.warning ? (
            <p>
              {plan.copyCount} to copy, {plan.skipCount} already on the card.
              {plan.duplicateSkipped > 0
                ? ` ${plan.duplicateSkipped} look${plan.duplicateSkipped === 1 ? "s" : ""} like games already on the card (skipped).`
                : ""}
            </p>
          ) : null}
          {plan && plan.duplicates.length > 0 ? (
            <div className="review">
              <h3>
                {plan.duplicates.reduce((total, group) => total + group.incoming.length, 0)} file
                {plan.duplicates.reduce((total, group) => total + group.incoming.length, 0) === 1
                  ? " looks"
                  : "s look"}{" "}
                like {plan.duplicates.length === 1 ? "a game" : "games"} already on the card
              </h3>
              <p>
                Keep the checkbox ticked to copy a file anyway. Unticked files stay off the
                card; nothing already on the card is touched.
              </p>
              <div className="row">
                <button disabled={busy} onClick={() => void planWithSmart()}>
                  Re-preview with these choices
                </button>
              </div>
              <table>
                <thead>
                  <tr>
                    <th>Already on the card</th>
                    <th>Incoming</th>
                    <th>Match</th>
                    <th>Copy anyway</th>
                  </tr>
                </thead>
                <tbody>
                  {plan.duplicates.map((group) =>
                    group.incoming.map((item) => (
                      <tr key={item.relativeDest}>
                        <td className="path">{group.resident}</td>
                        <td className="path">{item.relativeDest}</td>
                        <td>{`${Math.round(item.similarity * 100)}%`}</td>
                        <td>
                          <input
                            type="checkbox"
                            checked={dedupeKeep.includes(item.relativeDest)}
                            onChange={() => toggleDedupeKeep(item.relativeDest)}
                          />
                        </td>
                      </tr>
                    )),
                  )}
                </tbody>
              </table>
            </div>
          ) : null}
          {progress ? <p>{progress}</p> : null}
          {plan ? (
            <ul className="files">
              {plan.files.slice(0, 12).map((file) => (
                <li key={file.relativeDest}>
                  {file.action === "skip" ? "skip" : "copy"} {file.relativeDest}
                </li>
              ))}
            </ul>
          ) : null}
        </section>
      ) : null}

      {step === "bootOnce" && profile ? (
        <section>
          <h2>Boot the handheld once</h2>
          <ol>
            <li>The OS image matched the card.</li>
            <li>Eject the card and boot the RGB10X.</li>
            <li>Wait until the game menu appears. The handheld formats EASYROMS during this boot.</li>
            <li>Shut down from the menu, then put the card back in the reader.</li>
          </ol>
          <button disabled={busy} onClick={() => void findExpandedCard()}>
            The card is back
          </button>
        </section>
      ) : null}

      {step === "done" && flashLog && !report ? (
        <section>
          <h2>OS image verified</h2>
          <ol>
            <li>The card matched the image after the write.</li>
            <li>Eject the OS card and put it in the RGB10X OS slot.</li>
            <li>Leave the game card in the other slot.</li>
          </ol>
        </section>
      ) : null}

      {step === "done" && report && volume ? (
        <section>
          <h2>Safe to eject</h2>
          <ol>
            <li>
              Copied {report.copied} files ({formatBytes(report.bytesCopied)}). Skipped {report.skipped} unchanged files.
            </li>
            <li>Eject {volume.letter}: from Windows.</li>
            <li>Insert the card into the handheld.</li>
            <li>
              {keepExistingFolders
                ? "The files you copied should still be there. First boot has already run."
                : "Boot once. This flow does not flash firmware and does not disarm firstboot."}
            </li>
          </ol>
        </section>
      ) : null}
    </main>
  );
}
