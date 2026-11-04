import type { Settings } from "./api";

/**
 * App shortcuts. Each has a default that `settings.keybinds` can override by id ("" turns it off).
 * A shortcut is written like "Mod+Shift+P": modifiers in the order Mod (⌘), Ctrl, Alt (⌥), Shift, then a key.
 */
export const ACTIONS = [
  { id: "palette", label: "Command palette", key: "Mod+K" },
  { id: "switchProject", label: "Switch project", key: "Mod+O" },
  { id: "newPost", label: "New post", key: "Mod+N" },
  { id: "sync", label: "Sync with GitHub", key: "Mod+R" },
  { id: "settings", label: "Settings", key: "Mod+," },
  { id: "pull", label: "Pull", key: "Mod+Shift+L" },
  { id: "push", label: "Push", key: "Mod+Shift+P" },
  { id: "openPr", label: "Open PR", key: "" },
  { id: "review", label: "Review changes", key: "" },
  { id: "markers", label: "Toggle change markers", key: "Mod+Shift+D" },
] as const;

export type ActionId = (typeof ACTIONS)[number]["id"];

/** Editing and window shortcuts that a binding may not take over. */
const RESERVED = ["Mod+Q", "Mod+W", "Mod+H", "Mod+M", "Mod+A", "Mod+C", "Mod+V", "Mod+X", "Mod+Z", "Mod+Shift+Z"];

const CODE_KEYS: Record<string, string> = {
  Comma: ",", Period: ".", Slash: "/", Semicolon: ";", Quote: "'", BracketLeft: "[", BracketRight: "]",
  Backslash: "\\", Minus: "-", Equal: "=", Backquote: "`", Enter: "Enter", Space: "Space",
  ArrowUp: "Up", ArrowDown: "Down", ArrowLeft: "Left", ArrowRight: "Right",
};

/** The shortcut a key press makes, or null for a bare modifier. Uses the physical key, so Shift+, stays ",". */
export function fromEvent(e: KeyboardEvent): string | null {
  const key = /^Key([A-Z])$/.exec(e.code)?.[1] ?? /^Digit(\d)$/.exec(e.code)?.[1] ?? (/^F\d{1,2}$/.test(e.code) ? e.code : CODE_KEYS[e.code]);
  if (!key) return null;
  const mods = [e.metaKey && "Mod", e.ctrlKey && "Ctrl", e.altKey && "Alt", e.shiftKey && "Shift"].filter(Boolean);
  return [...mods, key].join("+");
}

/** Why a shortcut can't be used, or null when it can. */
export function problem(binding: string): string | null {
  const parts = binding.split("+");
  const key = parts[parts.length - 1];
  if (!parts.includes("Mod") && !parts.includes("Ctrl") && !/^F\d{1,2}$/.test(key)) return "Include ⌘ or ⌃, so typing isn't caught";
  if (RESERVED.includes(binding)) return `${display(binding)} is kept for editing and the window`;
  return null;
}

const SYMBOLS: Record<string, string> = { Mod: "⌘", Ctrl: "⌃", Alt: "⌥", Shift: "⇧", Enter: "↩", Space: "Space", Up: "↑", Down: "↓", Left: "←", Right: "→" };

/** "Mod+Shift+P" as "⌘⇧P". */
export function display(binding: string) {
  return binding ? binding.split("+").map((p) => SYMBOLS[p] ?? p).join("") : "";
}

/** The shortcut of every action. An override that is unusable falls back to the default. */
export function bindings(settings: Settings | null): Record<ActionId, string> {
  const out = {} as Record<ActionId, string>;
  for (const a of ACTIONS) {
    const o = settings?.keybinds[a.id];
    out[a.id] = o === undefined || (o && problem(o)) ? a.key : o;
  }
  return out;
}

/** Set while the settings page records a shortcut, so the app doesn't run the one being pressed. */
export const recording = { current: false };
