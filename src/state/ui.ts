/** Window-level dialogs that can be opened from anywhere (title bar, shortcuts, menus). */
import { create } from 'zustand';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import type { Snippet } from '@/lib/ipc/bindings/Snippet';

export type SiteDialogRequest =
  { mode: 'new'; initial?: SiteInput } | { mode: 'edit'; siteId: string };

/** A snippet that asks before running, waiting for the user's answer. */
export type RunConfirmation = {
  connId: string;
  snippet: Snippet;
  newTab: boolean;
  preview: string;
};

type UiState = {
  /** The command palette (Mod+K). */
  paletteOpen: boolean;
  /** The Manage snippets dialog, for the site of a connection tab (or globals only). */
  snippetsDialog: { siteId: string | null; siteName: string | null } | null;
  runConfirmation: RunConfirmation | null;
  setPaletteOpen: (open: boolean) => void;
  openSnippetsDialog: (request: UiState['snippetsDialog']) => void;
  setRunConfirmation: (request: RunConfirmation | null) => void;
  settingsOpen: boolean;
  helpOpen: boolean;
  siteDialog: SiteDialogRequest | null;
  setSettingsOpen: (open: boolean) => void;
  setHelpOpen: (open: boolean) => void;
  openSiteDialog: (request: SiteDialogRequest | null) => void;
};

export const useUi = create<UiState>()((set) => ({
  paletteOpen: false,
  snippetsDialog: null,
  runConfirmation: null,
  setPaletteOpen: (open) => set({ paletteOpen: open }),
  openSnippetsDialog: (request) => set({ snippetsDialog: request }),
  setRunConfirmation: (request) => set({ runConfirmation: request }),
  settingsOpen: false,
  helpOpen: false,
  siteDialog: null,
  setSettingsOpen: (open) => set({ settingsOpen: open }),
  setHelpOpen: (open) => set({ helpOpen: open }),
  openSiteDialog: (request) => set({ siteDialog: request }),
}));
