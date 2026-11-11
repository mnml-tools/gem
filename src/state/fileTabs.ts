/**
 * File tabs of each connection tab (plan §2.3, §3.3, §3.4). A *preview* tab (Space, single
 * open) is replaced by the next preview until it is edited or pinned (Enter, double click,
 * "Keep open"), as in VS Code. The document text itself lives in the editor (CodeView);
 * this store holds what the tab strip, the footer and the close guards need.
 */
import { create } from 'zustand';
import type { CommandExit } from '@/lib/ipc/bindings/CommandExit';
import type { ErrorCode } from '@/lib/ipc/bindings/ErrorCode';
import type { OpenedFile } from '@/lib/ipc/bindings/OpenedFile';
import { detectLanguage, type LanguageId } from '@/lib/languages';
import { baseName } from '@/lib/paths';

export type FileTabStatus =
  /** Waiting for `open_file`. */
  | 'loading'
  /** Larger than the confirmation threshold: asks before the first load. */
  | 'confirm'
  | 'ready'
  | 'error';

/** Metadata of an opened file, without its text (the editor owns the text). */
export type FileMeta = Omit<OpenedFile, 'text'>;

export type CommandRunState = 'running' | 'exited' | 'failed' | 'stopped';

/** A command output tab (ADR 0014): what ran, and how it ended. */
export type CommandTabInfo = {
  /** The command as typed or saved, `{dir}` unexpanded. */
  command: string;
  snippetId: string | null;
  /** The file list's folder when it ran (`{dir}`). */
  dir: string | null;
  /** The command as sent, after `{dir}` expansion (from `run_command`). */
  sent: string | null;
  runId: string | null;
  state: CommandRunState;
  exit: CommandExit | null;
  error: string | null;
  startedAt: number;
  durationMs: number | null;
  /** Bumped by each (re)run: events of an older run are ignored. */
  generation: number;
};

export type FileTab = {
  /** Unique within the connection tab: the remote path. */
  key: string;
  path: string;
  name: string;
  preview: boolean;
  status: FileTabStatus;
  meta: FileMeta | null;
  /** Size from the listing, used before the file is opened (confirmation, footer). */
  knownSize: number | null;
  /** Text of the latest (re)load, handed to the editor when `docVersion` changes. */
  initialText: string;
  docVersion: number;
  language: LanguageId;
  languageManual: boolean;
  dirty: boolean;
  /** A save or load is running. */
  busy: boolean;
  /** Opened whole ("Load all"): a reload asks for the whole file again. */
  whole: boolean;
  error: { code: ErrorCode; message: string } | null;
  /** Set for command output tabs: never dirty, never saved. */
  command?: CommandTabInfo;
};

type ConnFiles = { tabs: FileTab[]; activeKey: string | null };

type FileTabsState = {
  byConn: Record<string, ConnFiles>;
  /** Opens `path` (or focuses it). Returns the tab key and whether it must be loaded. */
  open: (
    connId: string,
    path: string,
    options: { preview: boolean; knownSize?: number | null; needsConfirm?: boolean },
  ) => { key: string; load: boolean };
  pin: (connId: string, key: string) => void;
  select: (connId: string, key: string) => void;
  close: (connId: string, key: string) => void;
  closeAll: (connId: string) => void;
  update: (connId: string, key: string, patch: Partial<FileTab>) => void;
  /** Applies a fresh `open_file` result: replaces the document and its metadata. */
  loaded: (connId: string, key: string, file: OpenedFile, whole: boolean) => void;
  /** Opens a command output tab after the active tab and selects it. */
  openCommand: (connId: string, title: string, info: CommandTabInfo) => string;
  updateCommand: (connId: string, key: string, patch: Partial<CommandTabInfo>) => void;
};

let commandCounter = 0;

const EMPTY: ConnFiles = { tabs: [], activeKey: null };

function newTab(
  path: string,
  preview: boolean,
  knownSize: number | null,
  confirm: boolean,
): FileTab {
  return {
    key: path,
    path,
    name: baseName(path),
    preview,
    status: confirm ? 'confirm' : 'loading',
    meta: null,
    knownSize,
    initialText: '',
    docVersion: 0,
    language: detectLanguage(path),
    languageManual: false,
    dirty: false,
    busy: false,
    whole: false,
    error: null,
  };
}

function patchTabs(
  state: FileTabsState,
  connId: string,
  change: (files: ConnFiles) => ConnFiles,
): Pick<FileTabsState, 'byConn'> {
  return { byConn: { ...state.byConn, [connId]: change(state.byConn[connId] ?? EMPTY) } };
}

export const useFileTabs = create<FileTabsState>()((set, get) => ({
  byConn: {},

  open: (connId, path, { preview, knownSize = null, needsConfirm = false }) => {
    const files = get().byConn[connId] ?? EMPTY;
    const existing = files.tabs.find((tab) => tab.key === path);
    if (existing) {
      set((state) =>
        patchTabs(state, connId, (current) => ({
          activeKey: path,
          tabs: current.tabs.map((tab) =>
            tab.key === path && !preview ? { ...tab, preview: false } : tab,
          ),
        })),
      );
      return { key: path, load: existing.status === 'error' };
    }
    const tab = newTab(path, preview, knownSize, needsConfirm);
    set((state) =>
      patchTabs(state, connId, (current) => {
        const tabs = [...current.tabs];
        // A new preview replaces the previous preview tab in place (it is never dirty: an
        // edit pins it). A pinned open goes after the active tab.
        const previewIndex = preview ? tabs.findIndex((item) => item.preview) : -1;
        if (previewIndex >= 0) tabs.splice(previewIndex, 1, tab);
        else {
          const activeIndex = tabs.findIndex((item) => item.key === current.activeKey);
          tabs.splice(activeIndex >= 0 ? activeIndex + 1 : tabs.length, 0, tab);
        }
        return { tabs, activeKey: path };
      }),
    );
    return { key: path, load: !needsConfirm };
  },

  pin: (connId, key) =>
    set((state) =>
      patchTabs(state, connId, (current) => ({
        ...current,
        tabs: current.tabs.map((tab) => (tab.key === key ? { ...tab, preview: false } : tab)),
      })),
    ),

  select: (connId, key) =>
    set((state) => patchTabs(state, connId, (current) => ({ ...current, activeKey: key }))),

  close: (connId, key) =>
    set((state) =>
      patchTabs(state, connId, (current) => {
        const index = current.tabs.findIndex((tab) => tab.key === key);
        if (index < 0) return current;
        const tabs = current.tabs.filter((tab) => tab.key !== key);
        let activeKey = current.activeKey;
        if (activeKey === key) activeKey = (tabs[index] ?? tabs[index - 1])?.key ?? null;
        return { tabs, activeKey };
      }),
    ),

  closeAll: (connId) =>
    set((state) => {
      const byConn = { ...state.byConn };
      delete byConn[connId];
      return { byConn };
    }),

  update: (connId, key, patch) =>
    set((state) =>
      patchTabs(state, connId, (current) => ({
        ...current,
        tabs: current.tabs.map((tab) => (tab.key === key ? { ...tab, ...patch } : tab)),
      })),
    ),

  openCommand: (connId, title, info) => {
    commandCounter += 1;
    const key = `cmd:${commandCounter}`;
    const tab: FileTab = {
      ...newTab('', false, null, false),
      key,
      name: title,
      status: 'ready',
      language: 'log',
      command: info,
    };
    set((state) =>
      patchTabs(state, connId, (current) => {
        const tabs = [...current.tabs];
        const activeIndex = tabs.findIndex((item) => item.key === current.activeKey);
        tabs.splice(activeIndex >= 0 ? activeIndex + 1 : tabs.length, 0, tab);
        return { tabs, activeKey: key };
      }),
    );
    return key;
  },

  updateCommand: (connId, key, patch) =>
    set((state) =>
      patchTabs(state, connId, (current) => ({
        ...current,
        tabs: current.tabs.map((tab) =>
          tab.key === key && tab.command ? { ...tab, command: { ...tab.command, ...patch } } : tab,
        ),
      })),
    ),

  loaded: (connId, key, file, whole) =>
    set((state) =>
      patchTabs(state, connId, (current) => ({
        ...current,
        tabs: current.tabs.map((tab) => {
          if (tab.key !== key) return tab;
          const { text, ...meta } = file;
          const firstLine = text.slice(0, Math.max(0, text.indexOf('\n'))) || text.slice(0, 200);
          return {
            ...tab,
            status: 'ready',
            meta,
            knownSize: file.size,
            initialText: text,
            docVersion: tab.docVersion + 1,
            language: tab.languageManual ? tab.language : detectLanguage(tab.path, firstLine),
            dirty: false,
            busy: false,
            whole,
            error: null,
          };
        }),
      })),
    ),
}));

export function getFileTabs(connId: string): ConnFiles {
  return useFileTabs.getState().byConn[connId] ?? EMPTY;
}

export function dirtyFileTabs(connId?: string): FileTab[] {
  const byConn = useFileTabs.getState().byConn;
  const groups = connId ? [byConn[connId] ?? EMPTY] : Object.values(byConn);
  return groups.flatMap((group) => group.tabs.filter((tab) => tab.dirty));
}

export function useConnFiles(connId: string): ConnFiles {
  return useFileTabs((state) => state.byConn[connId] ?? EMPTY);
}
