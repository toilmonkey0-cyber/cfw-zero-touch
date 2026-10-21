export const MANUAL_DEVICE_MESSAGE =
  "We couldn't tell which handheld this is. Please pick it from the list.";

export type StockDetection = {
  status: "matched" | "manual" | "none";
  profileId: string;
  device: string;
  screen: string;
  profileName: string;
  message: string;
};

export type DeviceNotice = {
  message: string;
  showConfirm: boolean;
  suggestedProfileId: string;
};

const emptyNotice: DeviceNotice = {
  message: "",
  showConfirm: false,
  suggestedProfileId: "",
};

/** Detection only pre-selects. Confirm is a separate step from showing the match. */
export function detectionNotice(
  detection: StockDetection | null,
  dismissed: boolean,
): DeviceNotice {
  if (!detection || dismissed || detection.status === "none") {
    return emptyNotice;
  }
  if (detection.status !== "matched") {
    return { ...emptyNotice, message: MANUAL_DEVICE_MESSAGE };
  }
  const identity =
    detection.screen.length > 0 ? `${detection.device} (${detection.screen})` : detection.device;
  return {
    message: `This looks like ${identity}. Confirm to use "${detection.profileName}", or choose a different layout.`,
    showConfirm: true,
    suggestedProfileId: detection.profileId,
  };
}

/** Returns a profile id only after the user confirms a match. */
export function profileAfterConfirm(
  detection: StockDetection | null,
  confirmed: boolean,
): string | null {
  if (!confirmed || !detection || detection.status !== "matched" || detection.profileId.length === 0) {
    return null;
  }
  return detection.profileId;
}
