export type LicenseView = {
  state: "active" | "inactive";
  reason: string;
  message: string;
  uses: number | null;
  overSoftLimit: boolean;
  stale: boolean;
  signedUpdates: boolean;
  support: boolean;
  safetyOpen: boolean;
};

/** Card-safety actions. A license view must not add or remove these. */
export const SAFETY_ACTIONS = ["backup", "restore", "probe", "fullcheck"] as const;

export const SAFETY_NOTE =
  "Backup, restore, card tests, and disk safeguards stay available without a license.";

export function safetyNote(): string {
  return SAFETY_NOTE;
}

export function safetyActions(): readonly string[] {
  return SAFETY_ACTIONS;
}

/** Prefer the backend message, but never repeat a key the operator just typed. */
export function statusText(message: string, typedKey: string): string {
  const trimmed = typedKey.trim();
  if (trimmed.length > 0 && message.includes(trimmed)) {
    return "Enter the license key from your Gumroad receipt to activate this computer.";
  }
  return message;
}

export function commercialUnlocked(view: LicenseView | null): {
  updates: boolean;
  support: boolean;
} {
  return {
    updates: Boolean(view?.signedUpdates),
    support: Boolean(view?.support),
  };
}
