/**
 * Typed wrappers for every Rust command. UI code calls these instead of `invoke` so argument
 * names and payload types are checked against the generated bindings
 * (docs/architecture.md#ipc-contract).
 */
import { Channel } from '@tauri-apps/api/core';
import { call } from './client';
import type { AppStatus } from './bindings/AppStatus';
import type { CommandEvent } from './bindings/CommandEvent';
import type { ConflictPolicy } from './bindings/ConflictPolicy';
import type { ConnectTarget } from './bindings/ConnectTarget';
import type { DeleteReport } from './bindings/DeleteReport';
import type { Entry } from './bindings/Entry';
import type { FileChunk } from './bindings/FileChunk';
import type { HostKeyInfo } from './bindings/HostKeyInfo';
import type { Listing } from './bindings/Listing';
import type { RunStarted } from './bindings/RunStarted';
import type { OpenedFile } from './bindings/OpenedFile';
import type { SavedFile } from './bindings/SavedFile';
import type { SaveFile } from './bindings/SaveFile';
import type { SecretUpdate } from './bindings/SecretUpdate';
import type { SessionEvent } from './bindings/SessionEvent';
import type { SessionInfo } from './bindings/SessionInfo';
import type { Settings } from './bindings/Settings';
import type { Site } from './bindings/Site';
import type { SiteInput } from './bindings/SiteInput';
import type { Snippet } from './bindings/Snippet';
import type { SnippetInput } from './bindings/SnippetInput';
import type { TestResult } from './bindings/TestResult';
import type { Transfer } from './bindings/Transfer';

/** Events emitted by Rust to the whole window (not per session). */
export const EVENTS = {
  /** Files from the OS dragged over the window: `DragHover`. */
  drag: 'gem://drag',
  /** Files from the OS dropped on the window: `DropPayload`. */
  drop: 'gem://drop',
} as const;

export const api = {
  // App
  appStatus: () => call<AppStatus>('app_status'),
  getSettings: () => call<Settings>('get_settings'),
  updateSettings: (settings: Settings) => call<Settings>('update_settings', { settings }),
  /** Native folder dialog; resolves to the updated settings (unchanged when cancelled). */
  chooseDownloadFolder: () => call<Settings>('choose_download_folder'),
  openDownloadFolder: () => call<void>('open_download_folder'),
  /** Native save dialog for a log; `false` when cancelled. */
  saveLog: (text: string, suggestedName: string) =>
    call<boolean>('save_log', { text, suggestedName }),
  /** Opens the latest release notes in the default browser. */
  openReleaseNotes: () => call<void>('open_release_notes'),

  // Sites
  listSites: () => call<Site[]>('list_sites'),
  createSite: (input: SiteInput, secret: string | null) =>
    call<Site>('create_site', { input, secret }),
  updateSite: (id: string, input: SiteInput, secret: SecretUpdate) =>
    call<Site>('update_site', { id, input, secret }),
  deleteSite: (id: string) => call<void>('delete_site', { id }),
  duplicateSite: (id: string) => call<Site>('duplicate_site', { id }),
  reorderSites: (ids: string[]) => call<void>('reorder_sites', { ids }),
  chooseKeyFile: () => call<string | null>('choose_key_file'),
  forgetHostKey: (host: string, port: number) => call<boolean>('forget_host_key', { host, port }),
  importSshConfig: () => call<SiteInput[]>('import_ssh_config'),
  importFilezilla: () => call<SiteInput[]>('import_filezilla'),
  exportSites: () => call<boolean>('export_sites'),
  importSitesFile: () => call<SiteInput[]>('import_sites_file'),

  // Sessions
  /** Opens a session; `onEvent` receives its log, state changes and transfers. */
  connect: (
    target: ConnectTarget,
    onEvent: (event: SessionEvent) => void,
    secret: string | null = null,
    remember = false,
  ) => {
    const events = new Channel<SessionEvent>();
    events.onmessage = onEvent;
    return call<SessionInfo>('connect', { target, secret, remember, events });
  },
  testConnection: (site: SiteInput, secret: string | null, siteId: string | null) =>
    call<TestResult>('test_connection', { site, secret, siteId }),
  trustHostKey: (key: HostKeyInfo) => call<void>('trust_host_key', { key }),
  disconnect: (sessionId: string) => call<void>('disconnect', { sessionId }),
  reconnect: (sessionId: string, secret: string | null = null) =>
    call<SessionInfo>('reconnect', { sessionId, secret }),

  // Files
  listDir: (sessionId: string, path: string) => call<Listing>('list_dir', { sessionId, path }),
  statPath: (sessionId: string, path: string) => call<Entry>('stat_path', { sessionId, path }),
  makeDir: (sessionId: string, path: string) => call<Entry>('make_dir', { sessionId, path }),
  makeFile: (sessionId: string, path: string) => call<Entry>('make_file', { sessionId, path }),
  renamePath: (sessionId: string, from: string, to: string) =>
    call<Entry>('rename_path', { sessionId, from, to }),
  deletePaths: (sessionId: string, paths: string[]) =>
    call<DeleteReport>('delete_paths', { sessionId, paths }),
  chmodPath: (sessionId: string, path: string, mode: number) =>
    call<Entry>('chmod_path', { sessionId, path, mode }),

  // Viewer and editor
  openFile: (sessionId: string, path: string, whole = false) =>
    call<OpenedFile>('open_file', { sessionId, path, whole }),
  readMore: (
    sessionId: string,
    file: Pick<OpenedFile, 'path' | 'nextOffset' | 'encoding' | 'size' | 'mtime'>,
    toEnd = false,
  ) =>
    call<FileChunk>('read_more', {
      sessionId,
      path: file.path,
      offset: file.nextOffset,
      encoding: file.encoding,
      expectedSize: file.size,
      expectedMtime: file.mtime,
      toEnd,
    }),
  saveFile: (sessionId: string, path: string, request: SaveFile) =>
    call<SavedFile>('save_file', { sessionId, path, request }),

  // Transfers
  downloadPaths: (sessionId: string, paths: string[], chooseFolder = false) =>
    call<Transfer[]>('download_paths', { sessionId, paths, chooseFolder }),
  uploadFiles: (sessionId: string, remoteDir: string, folders = false) =>
    call<Transfer[]>('upload_files', { sessionId, remoteDir, folders }),
  uploadDropped: (sessionId: string, dropId: string, remoteDir: string) =>
    call<Transfer[]>('upload_dropped', { sessionId, dropId, remoteDir }),
  cancelTransfer: (id: string) => call<void>('cancel_transfer', { id }),
  retryTransfer: (id: string) => call<void>('retry_transfer', { id }),
  clearTransfers: (sessionId: string) => call<void>('clear_transfers', { sessionId }),
  resolveConflict: (id: string, decision: Exclude<ConflictPolicy, 'ask'>, applyToAll: boolean) =>
    call<void>('resolve_conflict', { id, decision, applyToAll }),

  // Remote commands and snippets (SSH sessions only; ADR 0014)
  /** Global snippets plus those of `siteId` (globals first). */
  listSnippets: (siteId: string | null) => call<Snippet[]>('list_snippets', { siteId }),
  createSnippet: (input: SnippetInput) => call<Snippet>('create_snippet', { input }),
  updateSnippet: (id: string, input: SnippetInput) =>
    call<Snippet>('update_snippet', { id, input }),
  deleteSnippet: (id: string) => call<void>('delete_snippet', { id }),
  reorderSnippets: (ids: string[]) => call<void>('reorder_snippets', { ids }),
  /**
   * Runs a command on the session's SSH connection. `{dir}` is replaced by `dir`, shell-quoted.
   * `onEvent` receives the output, then exactly one `exit` or `failed` event.
   */
  runCommand: (
    sessionId: string,
    command: string,
    dir: string | null,
    onEvent: (event: CommandEvent) => void,
  ) => {
    const events = new Channel<CommandEvent>();
    events.onmessage = onEvent;
    return call<RunStarted>('run_command', { sessionId, command, dir, events });
  },
  cancelCommand: (runId: string) => call<void>('cancel_command', { runId }),
};
