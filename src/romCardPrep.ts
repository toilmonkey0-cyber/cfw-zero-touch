export type Invoke = <T>(command: string, args?: Record<string, unknown>) => Promise<T>;

export type RomVolume = {
  id: string;
  decision: "ready" | "needs_format" | "rejected";
  totalBytes: number;
  reason?: string;
};

export type RomCopyReport = {
  copied: number;
  skipped: number;
  bytesCopied: number;
};

export async function prepareAndSeed<V extends RomVolume>(
  invoke: Invoke,
  input: {
    profileId: string;
    volume: V;
    confirmation: string;
    displayedBytes: number;
  },
): Promise<{ volume: V; folders: string[] }> {
  const volume = await invoke<V>("prepare_card", {
    profileId: input.profileId,
    volumeId: input.volume.id,
    confirmation: input.volume.decision === "needs_format" ? input.confirmation : "",
    displayedBytes: input.displayedBytes,
  });
  const seeded = await invoke<{ folders: string[] }>("seed_card", {
    profileId: input.profileId,
    volumeId: input.volume.id,
  });
  return { volume, folders: seeded.folders };
}

export async function planRomCopy<T extends { copyCount: number }>(
  invoke: Invoke,
  input: { profileId: string; volumeId: string; library: string; include: string[] },
): Promise<T> {
  return invoke<T>("plan_roms", {
    profileId: input.profileId,
    volumeId: input.volumeId,
    library: input.library,
    include: input.include,
  });
}

export async function copyPreparedRoms(
  invoke: Invoke,
  input: { profileId: string; volumeId: string; library: string; include: string[] },
): Promise<RomCopyReport> {
  return invoke("copy_roms", {
    profileId: input.profileId,
    volumeId: input.volumeId,
    library: input.library,
    include: input.include,
    dryRun: false,
  });
}
