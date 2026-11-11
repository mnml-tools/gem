/**
 * Every keyboard shortcut of Gem, in one place: the kit's `useShortcut` definitions for the
 * window-wide ones, and the help table shown by the "Keyboard shortcuts" dialog (Mod+/).
 * Keys handled inside a focused widget (the file list, the editor) are listed for help only.
 */
import { formatShortcut, macPlatform, type Shortcut } from '@mnml-tools/ui';

// Window-wide keys that no text field or the editor uses also work from them (allowInInputs):
// most of the time the focus is in the code editor. Mod+W and Mod+L are captured before the
// editor by `useWindowShortcut` (Mod+W closes the file tab from the viewer; Mod+L would select
// a line in CodeMirror).
export const SHORTCUTS = {
  newTab: { key: 't', primary: true, allowInInputs: true },
  closeTab: { key: 'w', primary: true },
  togglePanel: { key: 'j', primary: true, allowInInputs: true },
  help: { key: '/', primary: true, allowInInputs: true },
  settings: { key: ',', primary: true, allowInInputs: true },
  refresh: { key: 'F5' },
  editPath: { key: 'l', primary: true },
  save: { key: 's', primary: true, allowInInputs: true },
  wrap: { key: 'z', alt: true, allowInInputs: true },
  palette: { key: 'k', primary: true, allowInInputs: true },
} as const satisfies Record<string, Shortcut>;

/** Mod+1 … Mod+9 select a connection tab. */
export const TAB_NUMBER_SHORTCUTS: Shortcut[] = Array.from({ length: 9 }, (_, index) => ({
  key: String(index + 1),
  primary: true,
  allowInInputs: true,
}));

/** Formats a shortcut for menus and tooltips, per OS (`Ctrl+S`, `⌘S`). */
export const keys = (shortcut: Shortcut) => formatShortcut(shortcut, macPlatform);

const ctrl = macPlatform ? '⌃' : 'Ctrl+';
const mod = macPlatform ? '⌘' : 'Ctrl+';
const alt = macPlatform ? '⌥' : 'Alt+';
const shift = macPlatform ? '⇧' : 'Shift+';

/** Mod+Enter, per OS (run in a new tab). */
export const MOD_ENTER = `${mod}Enter`;

export type ShortcutHelpSection = { title: string; items: { keys: string; label: string }[] };

export const SHORTCUT_HELP: ShortcutHelpSection[] = [
  {
    title: 'Tabs',
    items: [
      { keys: keys(SHORTCUTS.newTab), label: 'Open the Sites page in a new tab' },
      {
        keys: keys(SHORTCUTS.closeTab),
        label: 'Close the file tab (in the viewer) or the connection tab',
      },
      { keys: `${ctrl}Tab · ${ctrl}${shift}Tab`, label: 'Next / previous tab' },
      { keys: `${mod}1 … ${mod}9`, label: 'Go to tab 1 … 9' },
    ],
  },
  {
    title: 'File list',
    items: [
      { keys: 'Enter', label: 'Open the folder, or the file in a tab' },
      { keys: 'Space', label: 'Preview the file' },
      { keys: `Backspace · ${alt}↑`, label: 'Parent folder' },
      { keys: keys(SHORTCUTS.refresh), label: 'Refresh' },
      { keys: keys(SHORTCUTS.editPath), label: 'Edit the path' },
      { keys: 'F2', label: 'Rename' },
      { keys: macPlatform ? '⌘⌫ · Delete' : 'Delete', label: 'Delete' },
      { keys: `${mod}A`, label: 'Select all' },
      { keys: `${mod}C`, label: 'Copy path' },
      { keys: `${shift}F10`, label: 'Actions menu' },
      { keys: 'Type', label: 'Filter the folder' },
    ],
  },
  {
    title: 'Viewer and editor',
    items: [
      { keys: keys(SHORTCUTS.save), label: 'Save' },
      { keys: `${mod}F`, label: 'Find' },
      { keys: `${mod}G`, label: 'Go to line' },
      { keys: keys(SHORTCUTS.wrap), label: 'Toggle soft wrap' },
      { keys: 'Esc', label: 'Back to the file list' },
    ],
  },
  {
    title: 'Commands (SFTP)',
    items: [
      { keys: keys(SHORTCUTS.palette), label: 'Run a command or a snippet' },
      { keys: 'Enter', label: 'Run (reuses a finished tab of the same command)' },
      { keys: `${mod}Enter`, label: 'Run in a new tab' },
      { keys: `${alt}↑ · ${alt}↓`, label: 'Reorder snippets (Manage snippets)' },
    ],
  },
  {
    title: 'Window',
    items: [
      { keys: keys(SHORTCUTS.togglePanel), label: 'Show or hide the bottom panel' },
      { keys: keys(SHORTCUTS.settings), label: 'Settings' },
      { keys: keys(SHORTCUTS.help), label: 'Keyboard shortcuts' },
    ],
  },
];
