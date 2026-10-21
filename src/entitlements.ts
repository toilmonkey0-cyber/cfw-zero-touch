import { statusText, type LicenseView } from "./license";

/** Free with no key. These names are the product split from docs/LICENSING.md. */
export const FREE_FEATURES = [
  "rom-card-prep",
  "backup",
  "restore",
  "probe",
  "full-check",
  "guided-setup",
  "flash",
  "identity",
  "overwritten",
  "profile-feed",
] as const;

/** Paid. A missing or refused key opens the Enter key dialog. */
export const PAID_FEATURES = ["signed-updates", "support"] as const;

export type PaidFeature = (typeof PAID_FEATURES)[number];

export type LicenseDialog = "closed" | "unlock" | "manage";

export type LicenseUiState = {
  view: LicenseView | null;
  dialog: LicenseDialog;
  pending: PaidFeature | null;
  draftKey: string;
  busy: boolean;
  notice: string;
  unlockedPanel: PaidFeature | null;
};

export type LicenseAction =
  | { type: "status"; view: LicenseView }
  | { type: "manage" }
  | { type: "enter-key" }
  | { type: "draft"; key: string }
  | { type: "busy"; busy: boolean }
  | { type: "close" }
  | { type: "paid-result"; feature: PaidFeature; open: boolean; message: string }
  | { type: "activated"; view: LicenseView }
  | { type: "deactivated"; view: LicenseView }
  | { type: "replace"; state: LicenseUiState };

export type PaidAccess = { open: boolean; message: string };

export type PaidClient = {
  paid(feature: PaidFeature): Promise<PaidAccess>;
};

export function initialLicenseUi(): LicenseUiState {
  return {
    view: null,
    dialog: "closed",
    pending: null,
    draftKey: "",
    busy: false,
    notice: "",
    unlockedPanel: null,
  };
}

export function isFreeFeature(name: string): boolean {
  return (FREE_FEATURES as readonly string[]).includes(name);
}

export function paidFeatureLabel(feature: PaidFeature): string {
  return feature === "signed-updates" ? "Signed builds" : "Support";
}

export function paidPanelCopy(feature: PaidFeature, view: LicenseView | null): string {
  if (!view || view.state !== "active") return "";
  if (view.overSoftLimit) return view.message;
  if (feature === "signed-updates") {
    return "Signed builds are unlocked on this computer. This does not download a private update.";
  }
  return "Support is unlocked on this computer.";
}

function featureOpen(feature: PaidFeature | null, view: LicenseView): boolean {
  if (feature === "signed-updates") return view.signedUpdates;
  if (feature === "support") return view.support;
  return false;
}

function panelStillOpen(panel: PaidFeature | null, view: LicenseView): PaidFeature | null {
  return panel && featureOpen(panel, view) ? panel : null;
}

export function reduceLicense(state: LicenseUiState, action: LicenseAction): LicenseUiState {
  switch (action.type) {
    case "replace":
      return action.state;
    case "status":
      return {
        ...state,
        view: action.view,
        busy: false,
        unlockedPanel: panelStillOpen(state.unlockedPanel, action.view),
      };
    case "manage":
      return { ...state, dialog: "manage" };
    case "enter-key":
      return { ...state, dialog: "unlock", notice: "" };
    case "draft":
      return { ...state, draftKey: action.key };
    case "busy":
      return { ...state, busy: action.busy };
    case "close":
      return { ...state, dialog: "closed", busy: false, notice: "" };
    case "paid-result":
      if (action.open) {
        return {
          ...state,
          dialog: "closed",
          pending: action.feature,
          unlockedPanel: action.feature,
          busy: false,
          notice: action.message,
        };
      }
      return {
        ...state,
        dialog: "unlock",
        pending: action.feature,
        unlockedPanel: null,
        busy: false,
        notice: action.message,
      };
    case "activated": {
      const notice = statusText(action.view.message, state.draftKey);
      if (action.view.state !== "active") {
        return {
          ...state,
          view: action.view,
          busy: false,
          notice,
          dialog: "unlock",
          unlockedPanel: null,
        };
      }
      const pending = featureOpen(state.pending, action.view) ? state.pending : null;
      return {
        ...state,
        view: action.view,
        busy: false,
        notice: action.view.overSoftLimit ? action.view.message : "",
        draftKey: "",
        dialog: "closed",
        pending,
        unlockedPanel: pending,
      };
    }
    case "deactivated":
      return {
        ...state,
        view: action.view,
        busy: false,
        dialog: "manage",
        pending: null,
        unlockedPanel: null,
        draftKey: "",
        notice: statusText(action.view.message, state.draftKey),
      };
    default: {
      const unreachable: never = action;
      return unreachable;
    }
  }
}

export function applyStatus(state: LicenseUiState, view: LicenseView): LicenseUiState {
  return reduceLicense(state, { type: "status", view });
}

export async function requestPaidFeature(
  state: LicenseUiState,
  client: PaidClient,
  feature: PaidFeature,
): Promise<LicenseUiState> {
  const access = await client.paid(feature);
  return reduceLicense(state, {
    type: "paid-result",
    feature,
    open: access.open,
    message: access.message,
  });
}

export async function submitLicenseKey(
  state: LicenseUiState,
  client: { activate(key: string): Promise<LicenseView> },
): Promise<LicenseUiState> {
  try {
    const view = await client.activate(state.draftKey);
    return reduceLicense(state, { type: "activated", view });
  } catch (cause) {
    const message = statusText(cause instanceof Error ? cause.message : String(cause), state.draftKey);
    return reduceLicense(state, {
      type: "activated",
      view: {
        state: "inactive",
        reason: "unreachable",
        message,
        uses: null,
        overSoftLimit: false,
        stale: false,
        signedUpdates: false,
        support: false,
        safetyOpen: true,
      },
    });
  }
}

export async function deactivateThisComputer(
  state: LicenseUiState,
  client: { deactivate(): Promise<LicenseView> },
): Promise<LicenseUiState> {
  const view = await client.deactivate();
  return reduceLicense(state, { type: "deactivated", view });
}
