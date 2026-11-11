/**
 * Running remote commands in output tabs (ADR 0014): one `CommandOutput` per tab (module state,
 * appended in place), the tab's run state in the file-tab store, and the recent free commands of
 * each connection tab, in memory only (they may hold secrets typed by the user; never persisted).
 */
import { api } from '@/lib/ipc/api';
import type { CommandEvent } from '@/lib/ipc/bindings/CommandEvent';
import type { Snippet } from '@/lib/ipc/bindings/Snippet';
import { CommandOutput } from '@/lib/commandOutput';
import { errorMessage, sentence } from '@/lib/errors';
import { useFileTabs, type CommandTabInfo } from '@/state/fileTabs';
import type { RunConfirmation } from '@/state/ui';
import { getConnectionTab } from '@/state/workspace';

const outputs = new Map<string, CommandOutput>();
const recent = new Map<string, string[]>();

export const RECENT_LIMIT = 20;

/** Told when a run ends (exited, stopped or failed): the session and the folders it may have changed. */
type RunEndListener = (sessionId: string, dirs: string[]) => void;
const runEndListeners = new Set<RunEndListener>();

/** Subscribes to the end of every run; returns the unsubscribe. */
export function onRunEnd(listener: RunEndListener): () => void {
  runEndListeners.add(listener);
  return () => runEndListeners.delete(listener);
}

function ended(connId: string, sessionId: string, dir: string | null): void {
  // The folder it ran in, and the one shown now if the user moved on meanwhile.
  const now = getConnectionTab(connId)?.path ?? null;
  const dirs = [...new Set([dir, now].filter((item): item is string => !!item))];
  for (const listener of runEndListeners) listener(sessionId, dirs);
}

const id = (connId: string, key: string) => `${connId}\n${key}`;

export function commandOutput(connId: string, key: string): CommandOutput {
  let output = outputs.get(id(connId, key));
  if (!output) {
    output = new CommandOutput();
    outputs.set(id(connId, key), output);
  }
  return output;
}

/** Recent free commands of a connection tab, newest first. */
export function recentCommands(connId: string): string[] {
  return recent.get(connId) ?? [];
}

function remember(connId: string, command: string): void {
  const list = (recent.get(connId) ?? []).filter((item) => item !== command);
  list.unshift(command);
  recent.set(connId, list.slice(0, RECENT_LIMIT));
}

function commandTab(connId: string, key: string) {
  return useFileTabs.getState().byConn[connId]?.tabs.find((tab) => tab.key === key);
}

export type CommandSpec = {
  command: string;
  /** The tab title: the snippet's name, or the command. */
  title: string;
  snippetId: string | null;
};

/** Starts (or restarts) the run of a command tab. */
export async function runInTab(connId: string, key: string): Promise<void> {
  const tab = commandTab(connId, key);
  const connection = getConnectionTab(connId);
  const sessionId = connection?.session?.id;
  if (!tab?.command || !sessionId || connection?.status !== 'connected') return;
  const generation = tab.command.generation + 1;
  const dir = connection.path ?? connection.session?.startPath ?? null;
  const update = (patch: Partial<CommandTabInfo>) => {
    if (commandTab(connId, key)?.command?.generation === generation) {
      useFileTabs.getState().updateCommand(connId, key, patch);
    }
  };
  const output = commandOutput(connId, key);
  output.clear();
  useFileTabs.getState().updateCommand(connId, key, {
    generation,
    dir,
    state: 'running',
    exit: null,
    error: null,
    runId: null,
    sent: null,
    startedAt: Date.now(),
    durationMs: null,
  });
  const onEvent = (event: CommandEvent) => {
    if (commandTab(connId, key)?.command?.generation !== generation) return;
    switch (event.type) {
      case 'output':
        output.append(event.stream, event.text);
        return;
      case 'exit':
        update({
          state: event.exit.cancelled ? 'stopped' : 'exited',
          exit: event.exit,
          durationMs: event.durationMs,
        });
        ended(connId, sessionId, dir);
        return;
      case 'failed':
        update({ state: 'failed', error: sentence(event.error.message) });
        ended(connId, sessionId, dir);
    }
  };
  try {
    const started = await api.runCommand(sessionId, tab.command.command, dir, onEvent);
    update({ runId: started.runId, sent: started.command });
  } catch (cause) {
    update({ state: 'failed', error: errorMessage(cause) });
  }
}

/**
 * Runs a command. Unless `newTab`, a finished tab of the same command is reused (cleared);
 * otherwise a new output tab opens next to the active one.
 */
export function startCommand(connId: string, spec: CommandSpec, newTab = false): string | null {
  const files = useFileTabs.getState();
  if (!getConnectionTab(connId)?.session) return null;
  if (!spec.snippetId) remember(connId, spec.command);
  const reusable = newTab
    ? undefined
    : files.byConn[connId]?.tabs.find(
        (tab) =>
          tab.command &&
          tab.command.command === spec.command &&
          tab.command.snippetId === spec.snippetId &&
          tab.command.state !== 'running',
      );
  const key =
    reusable?.key ??
    files.openCommand(connId, spec.title, {
      command: spec.command,
      snippetId: spec.snippetId,
      dir: null,
      sent: null,
      runId: null,
      state: 'running',
      exit: null,
      error: null,
      startedAt: Date.now(),
      durationMs: null,
      generation: 0,
    });
  if (reusable) files.select(connId, key);
  void runInTab(connId, key);
  return key;
}

export async function stopCommand(connId: string, key: string): Promise<void> {
  const runId = commandTab(connId, key)?.command?.runId;
  if (runId) await api.cancelCommand(runId).catch(() => {});
}

/** Closes an output tab, stopping its command first if it still runs. */
export function closeCommandTab(connId: string, key: string): void {
  const tab = commandTab(connId, key);
  if (tab?.command?.state === 'running') void stopCommand(connId, key);
  outputs.delete(id(connId, key));
  useFileTabs.getState().close(connId, key);
}

/** The session dropped: running commands of that tab end as failed. */
export function failRunningCommands(connId: string, reason: string): void {
  const files = useFileTabs.getState();
  for (const tab of files.byConn[connId]?.tabs ?? []) {
    if (tab.command?.state === 'running') {
      files.updateCommand(connId, tab.key, { state: 'failed', error: reason });
    }
  }
}

/** A connection tab closes: stop its commands and forget its outputs and recent commands. */
export function forgetCommands(connId: string): void {
  for (const tab of useFileTabs.getState().byConn[connId]?.tabs ?? []) {
    if (tab.command?.state === 'running') void stopCommand(connId, tab.key);
  }
  for (const key of [...outputs.keys()]) if (key.startsWith(`${connId}\n`)) outputs.delete(key);
  recent.delete(connId);
}

/** `{dir}` as the server will see it (single-quoted), for the confirmation preview. */
export function previewCommand(command: string, dir: string | null): string {
  if (!command.includes('{dir}')) return command;
  const quoted = `'${(dir ?? '.').replace(/'/g, `'\\''`)}'`;
  return command.split('{dir}').join(quoted);
}

/** Runs a snippet, asking first when it says so (the dialog shows the expanded command). */
export function launchSnippet(
  connId: string,
  snippet: Snippet,
  newTab: boolean,
  confirm: (request: RunConfirmation) => void,
): void {
  if (snippet.confirm) {
    const dir = getConnectionTab(connId)?.path ?? null;
    confirm({ connId, snippet, newTab, preview: previewCommand(snippet.command, dir) });
    return;
  }
  startCommand(connId, snippetSpec(snippet), newTab);
}

export function snippetSpec(snippet: Snippet): CommandSpec {
  return { command: snippet.command, title: snippet.name, snippetId: snippet.id };
}
