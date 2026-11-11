/**
 * Dev-only in-memory implementation of every Rust command in `src/lib/ipc/api.ts`, so the UI
 * runs in a plain browser (`install.ts`) and in tests. It follows the IPC contract closely
 * enough for UI work: the same error codes (with `hostKey` for trust errors), session events
 * through the `connect` channel (log batches, state, transfers), latency, ranged reads,
 * conflict checks on save and transfers that progress over time.
 */
import type { AppStatus } from '@/lib/ipc/bindings/AppStatus';
import type { Capabilities } from '@/lib/ipc/bindings/Capabilities';
import type { ConflictPolicy } from '@/lib/ipc/bindings/ConflictPolicy';
import type { ConnectTarget } from '@/lib/ipc/bindings/ConnectTarget';
import type { DeleteReport } from '@/lib/ipc/bindings/DeleteReport';
import type { Entry } from '@/lib/ipc/bindings/Entry';
import type { ErrorCode } from '@/lib/ipc/bindings/ErrorCode';
import type { FileChunk } from '@/lib/ipc/bindings/FileChunk';
import type { HostKeyInfo } from '@/lib/ipc/bindings/HostKeyInfo';
import type { Listing } from '@/lib/ipc/bindings/Listing';
import type { LogKind } from '@/lib/ipc/bindings/LogKind';
import type { LogLine } from '@/lib/ipc/bindings/LogLine';
import type { OpenedFile } from '@/lib/ipc/bindings/OpenedFile';
import type { SavedFile } from '@/lib/ipc/bindings/SavedFile';
import type { SaveFile } from '@/lib/ipc/bindings/SaveFile';
import type { SecretUpdate } from '@/lib/ipc/bindings/SecretUpdate';
import type { SessionEvent } from '@/lib/ipc/bindings/SessionEvent';
import type { SessionInfo } from '@/lib/ipc/bindings/SessionInfo';
import type { Settings } from '@/lib/ipc/bindings/Settings';
import type { Site } from '@/lib/ipc/bindings/Site';
import type { SiteInput } from '@/lib/ipc/bindings/SiteInput';
import type { TestResult } from '@/lib/ipc/bindings/TestResult';
import type { Transfer } from '@/lib/ipc/bindings/Transfer';
import { baseName, joinPath, normalizePath, parentPath } from '@/lib/paths';
import type { CommandEvent } from '@/lib/ipc/bindings/CommandEvent';
import type { SnippetInput } from '@/lib/ipc/bindings/SnippetInput';
import { createCommandMock } from './mock-commands';
import { BIG_LOG_SIZE, BigLog, MockTree, storedSize, type FileNode, type Node } from './mock-tree';

/** Delivers one message to a `Channel` created by `api.connect`. */
export type ChannelSink = (message: SessionEvent) => void;

export type MockOptions = {
  /** Base latency of commands in ms (0 in tests). */
  latency?: number;
  /** `empty`: no saved sites (empty states). */
  mode?: 'demo' | 'empty';
  /** `false`: the OS keyring is missing (`?keyring=0`): nothing can be remembered. */
  keyring?: boolean;
  /** How `connect` messages reach the UI. */
  openChannel: (channel: unknown) => ChannelSink;
  /** Window events (`gem://drag`, `gem://drop`). */
  emit?: (event: string, payload: unknown) => void;
  log?: (message: string) => void;
};

type Session = {
  info: SessionInfo;
  sink: ChannelSink;
  seq: number;
  dropped: boolean;
  pendingLog: LogLine[];
  flush: ReturnType<typeof setTimeout> | null;
};

type Job = Transfer & { fail: boolean; target: string };

class MockError {
  constructor(
    readonly code: ErrorCode,
    readonly message: string,
    readonly hostKey: HostKeyInfo | null = null,
  ) {}
}

const fail = (code: ErrorCode, message: string, hostKey: HostKeyInfo | null = null): never => {
  throw new MockError(code, message, hostKey);
};

const now = () => Date.now();
const seconds = () => Math.floor(Date.now() / 1000);
let idCounter = 0;
const newId = (prefix: string) =>
  `${prefix}-${(idCounter += 1).toString(36)}-${Math.random().toString(36).slice(2, 8)}`;

const DEFAULT_SETTINGS: Settings = {
  downloadFolder: null,
  previewWindowKib: 2048,
  confirmAboveMib: 50,
  transferConcurrency: 2,
  keepaliveSecs: 30,
  showHidden: false,
  conflictPolicy: 'ask',
};

const FINGERPRINTS: Record<string, string> = {
  'prod.example.com:22': 'SHA256:pV6m2Ug0a7Wc1yqJ8eXvT9q8x4yHn3Bz5Kf0rLw2sQk',
  'staging.example.com:2222': 'SHA256:Qm9ja2luZy1zdGFnaW5nLWhvc3Qta2V5LWZvci1kZW1v',
  'backup.example.test:22': 'SHA256:N3wK3yAft3rR3inst4ll0fTh3B4ckupB0xD3m0nstr4t',
};

function fingerprintFor(host: string, port: number): string {
  const known = FINGERPRINTS[`${host}:${port}`];
  if (known) return known;
  let h = 2166136261;
  for (const char of `${host}:${port}`) h = Math.imul(h ^ char.charCodeAt(0), 16777619);
  const base = btoa(String((h >>> 0).toString(36)).repeat(5)).replace(/=+$/, '');
  return `SHA256:${base.slice(0, 43)}`;
}

function capabilities(protocol: SiteInput['protocol']): Capabilities {
  return protocol === 'sftp'
    ? { atomicRename: true, chmod: true, owner: true, symlinks: true, exec: true }
    : { atomicRename: true, chmod: true, owner: false, symlinks: false, exec: false };
}

function seedSites(mode: 'demo' | 'empty'): { sites: Site[]; secrets: Map<string, string> } {
  const secrets = new Map<string, string>();
  if (mode === 'empty') return { sites: [], secrets };
  const stamp = new Date(Date.now() - 20 * 86_400_000).toISOString();
  const base = {
    keyPath: null,
    ftpsMode: 'explicit' as const,
    passive: true,
    hasSecret: false,
    createdAt: stamp,
    updatedAt: stamp,
  };
  const sites: Site[] = [
    {
      ...base,
      id: 'site-prod',
      name: 'prod-api',
      protocol: 'sftp',
      host: 'prod.example.com',
      port: 22,
      user: 'deploy',
      auth: 'agent',
      initialPath: '/var/www/api',
      color: 'blue',
      sortOrder: 0,
    },
    {
      ...base,
      id: 'site-staging',
      name: 'staging',
      protocol: 'sftp',
      host: 'staging.example.com',
      port: 2222,
      user: 'deploy',
      auth: 'password',
      initialPath: '/var/www/api',
      color: 'green',
      sortOrder: 1,
    },
    {
      ...base,
      id: 'site-ftp',
      name: 'ftp-legacy',
      protocol: 'ftp',
      host: 'ftp.example.test',
      port: 21,
      user: 'legacy',
      auth: 'password',
      initialPath: '/var/www/api/legacy',
      color: 'orange',
      sortOrder: 2,
      hasSecret: true,
    },
    {
      ...base,
      id: 'site-backup',
      name: 'backup-box',
      protocol: 'sftp',
      host: 'backup.example.test',
      port: 22,
      user: 'backup',
      auth: 'key',
      keyPath: '/home/you/.ssh/id_ed25519',
      initialPath: null,
      color: 'purple',
      sortOrder: 3,
    },
  ];
  secrets.set('site-ftp', 'legacy-password');
  return { sites, secrets };
}

const LOCAL_FILES = ['report-2026-11.pdf', 'hero@2x.png', 'notes.txt', 'release.tar.gz'];

export function createMockBackend(options: MockOptions) {
  let latency = options.latency ?? 120;
  const keyring = options.keyring ?? true;
  const log = options.log ?? (() => {});
  const seeded = seedSites(options.mode ?? 'demo');
  let sites = seeded.sites;
  const secrets = seeded.secrets;
  let settings: Settings = { ...DEFAULT_SETTINGS };
  let tree = new MockTree();
  const bigLog = new BigLog();
  // ftp-legacy's server and staging's are already known; prod-api asks on first connect;
  // backup-box's key changed since it was stored.
  const knownHosts = new Map<string, string>([
    ['staging.example.com:2222', FINGERPRINTS['staging.example.com:2222']!],
    ['backup.example.test:22', 'SHA256:0ldK3yB3f0reR3inst4llW4sD1ff3r3ntFr0mN0w12'],
  ]);
  const sessions = new Map<string, Session>();
  const jobs = new Map<string, Job>();
  const downloaded = new Set<string>();
  const drops = new Map<string, string[]>();
  let ticker: ReturnType<typeof setInterval> | null = null;
  let applyToAll: Exclude<ConflictPolicy, 'ask'> | null = null;

  const shell = createCommandMock(options.mode ?? 'demo', {
    delay: () => latency,
    list: (dir) => {
      const resolved = tree.resolve(dir);
      if (resolved.node?.kind !== 'dir') return null;
      return tree
        .children(resolved.path)
        .map((child) => [baseName(child), tree.get(child)?.kind === 'dir'] as [string, boolean]);
    },
    log: (sessionId, kind, text) => {
      const session = sessions.get(sessionId);
      if (session) write(session, kind, text);
    },
    touch: (path) => {
      const target = normalizePath(path);
      if (tree.get(parentPath(target))?.kind !== 'dir') return false;
      const node = tree.get(target);
      if (node) node.mtime = seconds();
      else tree.file(target, '', { mtime: seconds() });
      return true;
    },
    remove: (path) => {
      const target = normalizePath(path);
      if (tree.get(target)?.kind !== 'file') return false;
      tree.remove(target);
      return true;
    },
  });

  const wait = (factor = 1) =>
    latency ? new Promise((resolve) => setTimeout(resolve, latency * factor)) : Promise.resolve();

  // ---- Session plumbing ----

  const sessionOf = (id: unknown): Session => {
    const session = sessions.get(String(id));
    if (!session) return fail('not_found', 'This session is closed.');
    if (session.dropped)
      return fail('disconnected', 'The connection was lost. Reconnect to continue.');
    return session;
  };

  const write = (session: Session, kind: LogKind, text: string) => {
    session.seq += 1;
    session.pendingLog.push({ seq: session.seq, ts: now(), kind, text });
    // Batched like the Rust side (~16 ms).
    session.flush ??= setTimeout(() => {
      session.flush = null;
      const lines = session.pendingLog;
      session.pendingLog = [];
      if (lines.length) session.sink({ type: 'log', lines });
    }, 16);
  };

  const sftp = (session: Session) => session.info.protocol === 'sftp';

  // ---- Entries ----

  const toEntry = (path: string, node: Node): Entry => {
    const base = { name: baseName(path), path, owner: node.owner, group: node.group };
    if (node.kind === 'symlink') {
      const target = tree.resolve(path).node;
      return {
        ...base,
        kind: 'symlink',
        size: null,
        mtime: node.mtime,
        mode: 0o777,
        linkTarget: node.target,
        targetKind: target ? (target.kind === 'symlink' ? null : target.kind) : null,
      };
    }
    return {
      ...base,
      kind: node.kind,
      size: node.kind === 'file' ? node.size : 4096,
      mtime: node.mtime,
      mode: node.mode,
      linkTarget: null,
      targetKind: null,
    };
  };

  const fileAt = (path: string): { path: string; node: FileNode } => {
    const resolved = tree.resolve(path);
    if (!resolved.node) return fail('not_found', `${path}: no such file`);
    if (resolved.node.kind === 'dir') return fail('validation', `${path} is a folder`);
    const parent = tree.get(parentPath(resolved.path));
    if (parent?.kind === 'dir' && parent.denied)
      return fail('permission_denied', `${path}: permission denied`);
    return { path: resolved.path, node: resolved.node as FileNode };
  };

  const windowOf = (node: FileNode, offset: number, length: number): string =>
    node.generated === 'biglog'
      ? bigLog.read(offset, length)
      : node.text.slice(offset, offset + length);

  // ---- Transfers ----

  const emitTransfers = (sessionId: string, changed: Job[]) => {
    const session = sessions.get(sessionId);
    if (!session || !changed.length) return;
    session.sink({
      type: 'transfers',
      transfers: changed.map(({ fail: _fail, target: _target, ...transfer }) => transfer),
    });
  };

  const finish = (job: Job) => {
    job.finishedAt = now();
    const session = sessions.get(job.sessionId);
    if (job.state === 'done' && job.direction === 'upload') {
      tree.file(job.target, `Uploaded from the browser preview: ${job.name}\n`, {
        size: job.size ?? 0,
        mtime: seconds(),
        binary: !/\.(txt|md|json|ya?ml|php|js|css|html?)$/.test(job.name),
      });
    }
    if (job.state === 'done' && job.direction === 'download') downloaded.add(job.name);
    if (session) {
      write(
        session,
        job.state === 'failed' ? 'error' : 'status',
        job.state === 'failed'
          ? `${job.direction === 'upload' ? 'Upload' : 'Download'} of ${job.name} failed: ${job.error}`
          : `${job.direction === 'upload' ? 'Uploaded' : 'Downloaded'} ${job.name} (${job.size} bytes)`,
      );
    }
  };

  const tick = () => {
    const bySession = new Map<string, Job[]>();
    const changed = (job: Job) => {
      const list = bySession.get(job.sessionId) ?? [];
      list.push(job);
      bySession.set(job.sessionId, list);
    };
    const all = [...jobs.values()];
    for (const sessionId of new Set(all.map((job) => job.sessionId))) {
      const session = sessions.get(sessionId);
      const mine = all.filter((job) => job.sessionId === sessionId);
      let running = mine.filter((job) => job.state === 'running').length;
      for (const job of mine) {
        if (session?.dropped && job.state === 'running') {
          job.state = 'failed';
          job.error = 'Connection lost';
          finish(job);
          changed(job);
          continue;
        }
        // Queued jobs wait while the connection is down and resume after `reconnect`.
        if (job.state === 'queued' && !session?.dropped && running < settings.transferConcurrency) {
          const exists =
            job.direction === 'download'
              ? downloaded.has(job.name)
              : tree.get(job.target)?.kind === 'file';
          const policy = applyToAll ?? settings.conflictPolicy;
          if (exists && policy === 'ask') {
            job.state = 'conflict';
            changed(job);
            continue;
          }
          if (exists && policy === 'skip') {
            job.state = 'skipped';
            finish(job);
            changed(job);
            continue;
          }
          if (exists && policy === 'rename') {
            const dot = job.name.lastIndexOf('.');
            job.name =
              dot > 0 ? `${job.name.slice(0, dot)} (1)${job.name.slice(dot)}` : `${job.name} (1)`;
            job.target = joinPath(parentPath(job.target), job.name);
            if (job.direction === 'upload') job.remotePath = job.target;
          }
          job.state = 'running';
          job.startedAt = now();
          running += 1;
          changed(job);
          continue;
        }
        if (job.state === 'running') {
          const size = job.size ?? 0;
          const step = Math.max(64 * 1024, Math.round(size / 18));
          job.done = Math.min(size, job.done + step + Math.floor(Math.random() * step * 0.3));
          if (job.fail && job.done > size * 0.45) {
            job.state = 'failed';
            job.error = 'Connection reset by peer';
            job.fail = false;
            finish(job);
          } else if (job.done >= size) {
            job.state = 'done';
            finish(job);
          }
          changed(job);
        }
      }
    }
    for (const [sessionId, list] of bySession) emitTransfers(sessionId, list);
    if (![...jobs.values()].some((job) => job.state === 'queued' || job.state === 'running')) {
      if (ticker) clearInterval(ticker);
      ticker = null;
    }
  };

  const startTicker = () => {
    ticker ??= setInterval(tick, latency ? 250 : 10);
  };

  const enqueue = (
    session: Session,
    specs: Omit<
      Job,
      'id' | 'sessionId' | 'done' | 'state' | 'error' | 'startedAt' | 'finishedAt'
    >[],
  ) => {
    const created = specs.map((spec): Job => ({
      ...spec,
      id: newId('transfer'),
      sessionId: session.info.id,
      done: 0,
      state: 'queued',
      error: null,
      startedAt: null,
      finishedAt: null,
    }));
    for (const job of created) jobs.set(job.id, job);
    startTicker();
    return created.map(({ fail: _fail, target: _target, ...transfer }) => transfer);
  };

  const downloadFolder = () => settings.downloadFolder ?? '/home/you/Downloads';

  const filesUnder = (path: string): { path: string; node: FileNode }[] => {
    const resolved = tree.resolve(path);
    if (resolved.node?.kind === 'file') return [{ path: resolved.path, node: resolved.node }];
    if (resolved.node?.kind !== 'dir') return [];
    return tree.children(resolved.path).flatMap((child) => filesUnder(child));
  };

  // ---- Sites ----

  const siteById = (id: unknown): Site => {
    const site = sites.find((item) => item.id === id);
    if (!site) return fail('not_found', 'This site no longer exists.');
    return site;
  };

  const checkInput = (input: SiteInput) => {
    if (!input.host.trim()) fail('validation', 'Enter a host name or address');
    if (input.port < 1 || input.port > 65535) fail('validation', 'Use a port from 1 to 65535');
  };

  // ---- Connect ----

  type Target = { site: SiteInput; siteId: string | null };

  const resolveTarget = (target: ConnectTarget): Target => {
    if (target.kind === 'adhoc') return { site: target.site, siteId: null };
    const site = siteById(target.id);
    return { site, siteId: site.id };
  };

  /** Runs the handshake checks; returns the conversation, or throws like Rust would. */
  const handshake = (
    { site, siteId }: Target,
    secret: string | null,
    remember: boolean,
    say: (kind: LogKind, text: string) => void,
  ): { serverInfo: string; home: string } => {
    const address = `${site.host}:${site.port}`;
    say('status', `Resolving ${site.host}…`);
    say('status', `Connecting to 203.0.113.${10 + (site.host.length % 40)}:${site.port}…`);
    if (/unreachable|offline/.test(site.host)) {
      say('error', `Could not connect to ${address}: connection timed out after 15 s`);
      fail('connection', `Could not connect to ${address}: connection timed out`);
    }
    const tls = site.protocol === 'ftps';
    if (site.protocol === 'sftp' || tls) {
      const fingerprint = fingerprintFor(site.host, site.port);
      const known = knownHosts.get(address);
      const keyType = tls ? 'x509' : 'ssh-ed25519';
      if (known && known !== fingerprint) {
        say('error', `Host key for ${address} does not match the stored key (${known})`);
        fail('host_key_changed', `The host key of ${address} changed.`, {
          host: site.host,
          port: site.port,
          keyType,
          fingerprint,
          previousFingerprint: known,
        });
      }
      if (!known) {
        say(
          'status',
          `${tls ? 'Certificate' : 'Host key'} of ${address} is not trusted yet (${fingerprint})`,
        );
        fail(tls ? 'certificate_untrusted' : 'host_key_unknown', `${address} is not trusted yet.`, {
          host: site.host,
          port: site.port,
          keyType,
          fingerprint,
          previousFingerprint: null,
        });
      }
    }
    const serverInfo = site.protocol === 'sftp' ? 'SSH-2.0-OpenSSH_9.6p1' : '220 (vsFTPd 3.0.5)';
    say('response', serverInfo);
    if (site.protocol === 'sftp')
      say('status', `Host key ${fingerprintFor(site.host, site.port)} matches`);
    else if (tls) say('command', 'AUTH TLS');
    if (tls) say('response', '234 Proceed with negotiation.');

    const needsSecret = site.protocol !== 'sftp' || site.auth !== 'agent';
    const stored = siteId ? secrets.get(siteId) : undefined;
    const effective = secret ?? stored ?? null;
    if (needsSecret && effective == null && !(site.protocol === 'sftp' && site.auth === 'key')) {
      say('status', 'A password is needed');
      fail('secret_required', 'Enter the password to connect.');
    }
    if (
      site.protocol === 'sftp' &&
      site.auth === 'key' &&
      effective == null &&
      site.keyPath?.includes('encrypted')
    ) {
      fail('secret_required', 'Enter the key passphrase.');
    }
    if (site.protocol !== 'sftp') {
      say('command', `USER ${site.user || 'anonymous'}`);
      say('response', '331 Please specify the password.');
      say('command', 'PASS ****');
    } else {
      say(
        'status',
        site.auth === 'agent'
          ? 'Trying public keys from the SSH agent'
          : site.auth === 'key'
            ? `Using key file ${site.keyPath ?? ''}`
            : 'Trying password authentication',
      );
    }
    if (effective === 'wrong') {
      say(
        'error',
        site.protocol === 'sftp' ? 'Authentication failed (password)' : '530 Login incorrect.',
      );
      fail(
        'auth_failed',
        site.protocol === 'sftp' ? 'Authentication failed.' : '530 Login incorrect.',
      );
    }
    if (site.protocol !== 'sftp') say('response', '230 Login successful.');
    else say('status', `Authenticated as ${site.user}`);
    if (siteId && remember && secret && keyring) {
      secrets.set(siteId, secret);
      sites = sites.map((item) => (item.id === siteId ? { ...item, hasSecret: true } : item));
    }
    const home = site.user === 'root' ? '/root' : `/home/${site.user || 'ftp'}`;
    say('command', site.protocol === 'sftp' ? 'realpath .' : 'PWD');
    say('response', site.protocol === 'sftp' ? home : `257 "${home}" is the current directory`);
    return { serverInfo, home };
  };

  // ---- Commands ----

  const commands: Record<string, (args: Record<string, unknown>) => unknown> = {
    app_status: (): AppStatus => ({
      version: '0.1.0',
      platform: /Mac/.test(navigator.userAgent)
        ? 'macos'
        : /Windows/.test(navigator.userAgent)
          ? 'windows'
          : 'linux',
      dataError: null,
      keyringAvailable: keyring,
    }),
    get_settings: () => settings,
    update_settings: ({ settings: next }) => {
      const wanted = next as Settings;
      // As in Rust: a new download folder only comes from the native dialog.
      if (wanted.downloadFolder !== null && wanted.downloadFolder !== settings.downloadFolder) {
        return fail('validation', 'Choose the download folder with the folder dialog.');
      }
      settings = { ...wanted };
      return settings;
    },
    choose_download_folder: () => {
      settings = { ...settings, downloadFolder: '/home/you/Downloads/gem' };
      return settings;
    },
    open_download_folder: () => {
      log(`open download folder: ${downloadFolder()}`);
      return null;
    },
    save_log: ({ suggestedName }) => {
      log(`save log as ${String(suggestedName)}`);
      return true;
    },

    list_sites: () => [...sites].sort((a, b) => a.sortOrder - b.sortOrder),
    create_site: ({ input, secret }) => {
      const site = input as SiteInput;
      checkInput(site);
      if (secret && !keyring) return fail('platform', 'The system keyring is not available.');
      const stamp = new Date().toISOString();
      const created: Site = {
        ...site,
        id: newId('site'),
        sortOrder: sites.length,
        hasSecret: !!secret,
        createdAt: stamp,
        updatedAt: stamp,
      };
      if (secret) secrets.set(created.id, String(secret));
      sites = [...sites, created];
      return created;
    },
    update_site: ({ id, input, secret }) => {
      const site = siteById(id);
      checkInput(input as SiteInput);
      const update = secret as SecretUpdate;
      if (update.kind === 'set' && !keyring) {
        return fail('platform', 'The system keyring is not available.');
      }
      let hasSecret = site.hasSecret;
      if (update.kind === 'clear') {
        secrets.delete(site.id);
        hasSecret = false;
      } else if (update.kind === 'set') {
        secrets.set(site.id, update.value);
        hasSecret = true;
      }
      const updated: Site = {
        ...site,
        ...(input as SiteInput),
        hasSecret,
        updatedAt: new Date().toISOString(),
      };
      sites = sites.map((item) => (item.id === site.id ? updated : item));
      return updated;
    },
    delete_site: ({ id }) => {
      siteById(id);
      shell.deleteSiteSnippets(String(id));
      sites = sites.filter((item) => item.id !== id);
      secrets.delete(String(id));
      return null;
    },
    duplicate_site: ({ id }) => {
      const site = siteById(id);
      const copy: Site = {
        ...site,
        id: newId('site'),
        name: `${site.name} copy`,
        hasSecret: false,
        sortOrder: sites.length,
      };
      sites = [...sites, copy];
      return copy;
    },
    reorder_sites: ({ ids }) => {
      const order = ids as string[];
      sites = sites.map((site) => ({ ...site, sortOrder: order.indexOf(site.id) }));
      return null;
    },
    choose_key_file: () => '/home/you/.ssh/id_ed25519',
    forget_host_key: ({ host, port }) => knownHosts.delete(`${String(host)}:${Number(port)}`),
    import_ssh_config: (): SiteInput[] => [
      {
        name: 'web-1',
        protocol: 'sftp',
        host: 'web-1.example.test',
        port: 22,
        user: 'deploy',
        auth: 'agent',
        keyPath: null,
        ftpsMode: 'explicit',
        passive: true,
        initialPath: null,
        color: null,
      },
      {
        name: 'db-bastion',
        protocol: 'sftp',
        host: '203.0.113.40',
        port: 2200,
        user: 'ops',
        auth: 'key',
        keyPath: '/home/you/.ssh/bastion',
        ftpsMode: 'explicit',
        passive: true,
        initialPath: null,
        color: null,
      },
    ],
    import_filezilla: (): SiteInput[] => [
      {
        name: 'Old shop',
        protocol: 'ftps',
        host: 'shop.example.test',
        port: 21,
        user: 'shop',
        auth: 'password',
        keyPath: null,
        ftpsMode: 'explicit',
        passive: true,
        initialPath: '/public_html',
        color: null,
      },
    ],
    import_sites_file: (): SiteInput[] => [],
    export_sites: () => true,

    connect: ({ target, secret, remember, events }) => {
      const resolved = resolveTarget(target as ConnectTarget);
      const lines: LogLine[] = [];
      let seq = 0;
      const say = (kind: LogKind, text: string) =>
        lines.push({ seq: (seq += 1), ts: now(), kind, text });
      const sink = options.openChannel(events);
      try {
        const { serverInfo, home } = handshake(
          resolved,
          (secret as string | null) ?? null,
          !!remember,
          say,
        );
        const { site, siteId } = resolved;
        const start = site.initialPath ? normalizePath(site.initialPath) : home;
        tree.dir(home, { owner: site.user || 'ftp' });
        const info: SessionInfo = {
          id: newId('session'),
          siteId,
          label: siteId ? site.name : `${site.user ? `${site.user}@` : ''}${site.host}`,
          protocol: site.protocol,
          host: site.host,
          port: site.port,
          user: site.user,
          home,
          startPath: start,
          insecure: site.protocol === 'ftp',
          serverInfo,
          capabilities: capabilities(site.protocol),
        };
        const session: Session = { info, sink, seq, dropped: false, pendingLog: [], flush: null };
        sessions.set(info.id, session);
        sink({ type: 'log', lines });
        sink({ type: 'state', state: 'connected', reason: null });
        return info;
      } catch (error) {
        // The failed handshake is still logged in the tab (CA-07).
        sink({ type: 'log', lines });
        throw error;
      }
    },
    test_connection: ({ site, secret, siteId }): TestResult => {
      const lines: LogLine[] = [];
      let seq = 0;
      const say = (kind: LogKind, text: string) =>
        lines.push({ seq: (seq += 1), ts: now(), kind, text });
      const input = site as SiteInput;
      // Invalid input still rejects; every connection-level failure resolves with `ok: false`.
      checkInput(input);
      try {
        const { serverInfo, home } = handshake(
          { site: input, siteId: (siteId as string | null) ?? null },
          (secret as string | null) ?? null,
          false,
          say,
        );
        say('status', 'Disconnected');
        return { ok: true, serverInfo, home, error: null, log: lines };
      } catch (error) {
        if (!(error instanceof MockError)) throw error;
        say('error', `Test failed: ${error.message}`);
        return {
          ok: false,
          serverInfo: null,
          home: null,
          error: { code: error.code, message: error.message, hostKey: error.hostKey },
          log: lines,
        };
      }
    },
    trust_host_key: ({ key }) => {
      const info = key as HostKeyInfo;
      knownHosts.set(`${info.host}:${info.port}`, info.fingerprint);
      return null;
    },
    disconnect: ({ sessionId }) => {
      const session = sessions.get(String(sessionId));
      if (session) {
        for (const job of jobs.values()) if (job.sessionId === session.info.id) jobs.delete(job.id);
        sessions.delete(session.info.id);
      }
      return null;
    },
    reconnect: ({ sessionId }) => {
      const session = sessions.get(String(sessionId));
      if (!session) return fail('not_found', 'This session is closed.');
      write(session, 'status', `Reconnecting to ${session.info.host}:${session.info.port}…`);
      write(session, 'response', session.info.serverInfo ?? '');
      write(session, 'status', `Authenticated as ${session.info.user}`);
      session.dropped = false;
      session.sink({ type: 'state', state: 'connected', reason: null });
      startTicker();
      return session.info;
    },

    list_dir: ({ sessionId, path }) => {
      const session = sessionOf(sessionId);
      const requested = normalizePath(String(path));
      write(session, 'command', sftp(session) ? `opendir ${requested}` : `CWD ${requested}`);
      const resolved = tree.resolve(requested);
      if (!resolved.node) {
        write(
          session,
          'error',
          sftp(session)
            ? `${requested}: No such file`
            : `550 ${requested}: No such file or directory.`,
        );
        return fail('not_found', `${requested}: no such folder`);
      }
      if (resolved.node.kind !== 'dir') return fail('validation', `${requested} is not a folder`);
      if (resolved.node.denied) {
        write(
          session,
          'error',
          sftp(session)
            ? `${requested}: Permission denied`
            : `550 ${requested}: Permission denied.`,
        );
        return fail('permission_denied', `${requested}: permission denied`);
      }
      if (!sftp(session)) {
        write(session, 'response', '250 Directory successfully changed.');
        write(session, 'command', 'MLSD');
      }
      const entries = tree
        .children(resolved.path)
        .map((child) => toEntry(child, tree.get(child)!))
        .map((entry) => (sftp(session) ? entry : { ...entry, owner: null, group: null }));
      write(
        session,
        'response',
        sftp(session)
          ? `${entries.length} entries`
          : `226 Transfer complete (${entries.length} entries).`,
      );
      const listing: Listing = { path: resolved.path, entries };
      return listing;
    },
    stat_path: ({ sessionId, path }) => {
      sessionOf(sessionId);
      const node = tree.get(String(path));
      if (!node) return fail('not_found', `${String(path)}: no such file`);
      return toEntry(normalizePath(String(path)), node);
    },
    make_dir: ({ sessionId, path }) => {
      const session = sessionOf(sessionId);
      const target = normalizePath(String(path));
      write(session, 'command', sftp(session) ? `mkdir ${target}` : `MKD ${target}`);
      if (tree.get(target)) return fail('validation', `${baseName(target)} already exists`);
      const parent = tree.get(parentPath(target));
      if (parent?.kind === 'dir' && parent.denied)
        return fail('permission_denied', 'Permission denied');
      tree.dir(target, { mtime: seconds() });
      write(session, 'response', sftp(session) ? 'OK' : `257 "${target}" created`);
      return toEntry(target, tree.get(target)!);
    },
    make_file: ({ sessionId, path }) => {
      const session = sessionOf(sessionId);
      const target = normalizePath(String(path));
      write(
        session,
        'command',
        sftp(session) ? `open ${target} (create, exclusive)` : `STOR ${target}`,
      );
      if (tree.get(target)) return fail('validation', `${baseName(target)} already exists`);
      tree.file(target, '', { mtime: seconds() });
      write(session, 'response', sftp(session) ? 'OK' : '226 Transfer complete.');
      return toEntry(target, tree.get(target)!);
    },
    rename_path: ({ sessionId, from, to }) => {
      const session = sessionOf(sessionId);
      const source = normalizePath(String(from));
      const target = normalizePath(String(to));
      write(
        session,
        'command',
        sftp(session) ? `posix-rename ${source} ${target}` : `RNFR ${source}`,
      );
      if (!tree.get(source)) return fail('not_found', `${source}: no such file`);
      if (tree.get(target)) return fail('validation', `${baseName(target)} already exists`);
      tree.move(source, target);
      if (!sftp(session)) write(session, 'command', `RNTO ${target}`);
      write(session, 'response', sftp(session) ? 'OK' : '250 Rename successful.');
      return toEntry(target, tree.get(target)!);
    },
    delete_paths: ({ sessionId, paths }): DeleteReport => {
      const session = sessionOf(sessionId);
      const report: DeleteReport = { deleted: 0, failed: [] };
      for (const raw of paths as string[]) {
        const path = normalizePath(raw);
        write(session, 'command', sftp(session) ? `remove ${path}` : `DELE ${path}`);
        const node = tree.get(path);
        const parent = tree.get(parentPath(path));
        if (!node) {
          report.failed.push({ path, message: 'No such file' });
          write(session, 'error', `${path}: No such file`);
        } else if (
          (node.kind === 'dir' && node.denied) ||
          (parent?.kind === 'dir' && parent.denied)
        ) {
          report.failed.push({ path, message: 'Permission denied' });
          write(session, 'error', `${path}: Permission denied`);
        } else {
          report.deleted += tree.remove(path);
        }
      }
      return report;
    },
    chmod_path: ({ sessionId, path, mode }) => {
      const session = sessionOf(sessionId);
      const target = normalizePath(String(path));
      const node = tree.get(target);
      write(
        session,
        'command',
        sftp(session)
          ? `setstat ${target} mode ${Number(mode).toString(8)}`
          : `SITE CHMOD ${Number(mode).toString(8)} ${target}`,
      );
      if (!node || node.kind === 'symlink') return fail('not_found', `${target}: no such file`);
      node.mode = Number(mode) & 0o7777;
      write(session, 'response', sftp(session) ? 'OK' : '200 SITE CHMOD command ok.');
      return toEntry(target, node);
    },

    open_file: ({ sessionId, path, whole }): OpenedFile => {
      const session = sessionOf(sessionId);
      const file = fileAt(String(path));
      const { node } = file;
      write(session, 'command', sftp(session) ? `open ${file.path} (read)` : `RETR ${file.path}`);
      const base = {
        path: normalizePath(String(path)),
        name: baseName(String(path)),
        size: node.size,
        mtime: node.mtime,
        bom: false,
        lineEnding: node.lineEnding,
        mixedLineEndings: node.mixed,
      };
      if (node.binary) {
        write(session, 'response', 'binary file: NUL bytes in the first window');
        return {
          ...base,
          text: '',
          nextOffset: Math.min(node.size, 4096),
          complete: false,
          binary: true,
          encoding: 'utf-8',
          readOnly: 'binary',
        };
      }
      if (whole && node.generated === 'biglog') {
        write(session, 'error', 'The browser preview serves at most 16 MB at once');
        return fail(
          'validation',
          'The browser preview can’t load all 200 MB at once; use Load more.',
        );
      }
      const length = whole ? node.size : settings.previewWindowKib * 1024;
      const text = windowOf(node, 0, length);
      const total = node.generated ? node.size : node.text.length;
      const loaded = node.generated ? Math.min(node.size, length) : Math.min(total, length);
      const complete = loaded >= total;
      write(session, 'response', `${loaded} of ${node.size} bytes`);
      return {
        ...base,
        text,
        nextOffset: complete ? node.size : loaded,
        complete,
        binary: false,
        encoding: node.encoding,
        readOnly: complete ? null : 'partial',
      };
    },
    read_more: ({ sessionId, path, offset, expectedSize, expectedMtime, toEnd }): FileChunk => {
      const session = sessionOf(sessionId);
      const { node, path: resolved } = fileAt(String(path));
      if (node.size !== expectedSize || node.mtime !== expectedMtime) {
        write(session, 'error', `${resolved} changed on the server since it was opened`);
        return fail(
          'conflict',
          'The file changed on the server. Reload it to see the new version.',
        );
      }
      const start = Number(offset);
      const limit = node.generated ? node.size : node.text.length;
      const length = toEnd ? limit - start : settings.previewWindowKib * 1024;
      const text = windowOf(node, start, length);
      const next = Math.min(limit, start + length);
      write(session, 'command', sftp(session) ? `read ${resolved} @${start}` : `REST ${start}`);
      return { text, nextOffset: next >= limit ? node.size : next, complete: next >= limit };
    },
    save_file: ({ sessionId, path, request }): SavedFile => {
      const session = sessionOf(sessionId);
      const save = request as SaveFile;
      const { node, path: resolved } = fileAt(String(path));
      write(session, 'command', sftp(session) ? `stat ${resolved}` : `MDTM ${resolved}`);
      if (!save.force && (node.size !== save.expectedSize || node.mtime !== save.expectedMtime)) {
        write(session, 'error', `${resolved} changed on the server since it was opened`);
        return fail('conflict', 'The file changed on the server since you opened it.');
      }
      const parent = tree.get(parentPath(resolved));
      const atomic = !(parent?.kind === 'dir' && parent.noTempFiles);
      if (!atomic && !save.allowInPlace) {
        write(
          session,
          'error',
          `${parentPath(resolved)}: cannot create a temporary file (permission denied)`,
        );
        return fail('unsupported', 'The folder does not allow a temporary file.');
      }
      const temp = `${parentPath(resolved)}/.${baseName(resolved)}.gem-${Math.random().toString(36).slice(2, 8)}`;
      if (atomic) {
        write(session, 'command', sftp(session) ? `open ${temp} (write)` : `STOR ${temp}`);
        write(
          session,
          'command',
          sftp(session) ? `posix-rename ${temp} ${resolved}` : `RNFR ${temp} / RNTO ${resolved}`,
        );
      } else
        write(
          session,
          'command',
          sftp(session) ? `open ${resolved} (write, truncate)` : `STOR ${resolved}`,
        );
      node.text = save.text;
      node.encoding = save.encoding;
      node.lineEnding = save.lineEnding;
      node.mixed = false;
      node.size = storedSize(save.text, save.encoding, save.lineEnding, false);
      node.mtime = Math.max(seconds(), node.mtime + 1);
      write(session, 'response', `Saved ${node.size} bytes${atomic ? '' : ' (in place)'}`);
      return { size: node.size, mtime: node.mtime, atomic };
    },

    download_paths: ({ sessionId, paths, chooseFolder }) => {
      const session = sessionOf(sessionId);
      const folder = chooseFolder ? '/home/you/Desktop' : downloadFolder();
      const files = (paths as string[]).flatMap((path) =>
        filesUnder(path).map((file) => ({ ...file, root: parentPath(normalizePath(path)) })),
      );
      write(
        session,
        'status',
        `Queued ${files.length} download${files.length === 1 ? '' : 's'} to ${folder}`,
      );
      return enqueue(
        session,
        files.map((file) => ({
          direction: 'download',
          name: baseName(file.path),
          localPath: `${folder}${file.path.slice(file.root === '/' ? 0 : file.root.length)}`,
          remotePath: file.path,
          size: file.node.size,
          fail: false,
          target: file.path,
        })),
      );
    },
    upload_files: ({ sessionId, remoteDir, folders }) => {
      const session = sessionOf(sessionId);
      const dir = normalizePath(String(remoteDir));
      const names = folders
        ? ['site-assets/app.css', 'site-assets/app.js', 'site-assets/logo.svg']
        : LOCAL_FILES;
      write(
        session,
        'status',
        `Queued ${names.length} upload${names.length === 1 ? '' : 's'} to ${dir}`,
      );
      return enqueue(
        session,
        names.map((name, index) => {
          if (name.includes('/')) tree.dir(joinPath(dir, parentPath(`/${name}`).slice(1)));
          const target = joinPath(dir, name);
          return {
            direction: 'upload',
            name: baseName(name),
            localPath: `/home/you/Documents/${name}`,
            remotePath: target,
            size: [1_840_000, 6_400_000, 3_100, 24_000_000][index % 4]!,
            // The archive fails once (Retry then succeeds).
            fail: name.endsWith('.tar.gz'),
            target,
          };
        }),
      );
    },
    upload_dropped: ({ sessionId, dropId, remoteDir }) => {
      const session = sessionOf(sessionId);
      const names = drops.get(String(dropId));
      if (!names) return fail('not_found', 'That drop has expired.');
      drops.delete(String(dropId));
      const dir = normalizePath(String(remoteDir));
      return enqueue(
        session,
        names.map((name) => ({
          direction: 'upload',
          name,
          localPath: `/home/you/Desktop/${name}`,
          remotePath: joinPath(dir, name),
          size: 900_000 + name.length * 31_000,
          fail: false,
          target: joinPath(dir, name),
        })),
      );
    },
    cancel_transfer: ({ id }) => {
      const job = jobs.get(String(id));
      if (job && (job.state === 'queued' || job.state === 'running' || job.state === 'conflict')) {
        job.state = 'cancelled';
        job.finishedAt = now();
        emitTransfers(job.sessionId, [job]);
      }
      return null;
    },
    retry_transfer: ({ id }) => {
      const job = jobs.get(String(id));
      if (job && (job.state === 'failed' || job.state === 'cancelled')) {
        job.state = 'queued';
        job.done = 0;
        job.error = null;
        emitTransfers(job.sessionId, [job]);
        startTicker();
      }
      return null;
    },
    clear_transfers: ({ sessionId }) => {
      for (const job of [...jobs.values()]) {
        if (
          job.sessionId === sessionId &&
          ['done', 'failed', 'cancelled', 'skipped'].includes(job.state)
        ) {
          jobs.delete(job.id);
        }
      }
      return null;
    },
    list_snippets: ({ siteId }) => shell.listSnippets((siteId as string | null) ?? null),
    create_snippet: ({ input }) => {
      const snippet = input as SnippetInput;
      if (!snippet.name.trim() || !snippet.command.trim()) {
        return fail('validation', 'A snippet needs a name and a command.');
      }
      return shell.createSnippet(snippet);
    },
    update_snippet: ({ id, input }) =>
      shell.updateSnippet(String(id), input as SnippetInput) ??
      fail('not_found', 'This snippet no longer exists.'),
    delete_snippet: ({ id }) => {
      shell.deleteSnippet(String(id));
      return null;
    },
    reorder_snippets: ({ ids }) => {
      shell.reorderSnippets(ids as string[]);
      return null;
    },
    run_command: ({ sessionId, command, dir, events }) => {
      const session = sessionOf(sessionId);
      if (!session.info.capabilities.exec) {
        return fail('unsupported', 'Commands need an SSH (SFTP) connection.');
      }
      const sink = options.openChannel(events) as unknown as (event: CommandEvent) => void;
      return shell.run(session.info.id, String(command), (dir as string | null) ?? null, sink);
    },
    cancel_command: ({ runId }) => {
      shell.cancel(String(runId));
      return null;
    },
    resolve_conflict: ({ id, decision, applyToAll: all }) => {
      const job = jobs.get(String(id));
      const choice = decision as Exclude<ConflictPolicy, 'ask'>;
      const targets = all
        ? [...jobs.values()].filter(
            (item) => item.state === 'conflict' && item.sessionId === job?.sessionId,
          )
        : job
          ? [job]
          : [];
      if (all) applyToAll = choice;
      for (const item of targets) {
        if (choice === 'skip') {
          item.state = 'skipped';
          item.finishedAt = now();
        } else {
          if (choice === 'rename') {
            const dot = item.name.lastIndexOf('.');
            item.name =
              dot > 0
                ? `${item.name.slice(0, dot)} (1)${item.name.slice(dot)}`
                : `${item.name} (1)`;
            item.target = joinPath(parentPath(item.target), item.name);
            if (item.direction === 'upload') item.remotePath = item.target;
          }
          if (item.direction === 'download') downloaded.delete(item.name);
          item.state = 'running';
          item.startedAt = now();
        }
      }
      if (job) emitTransfers(job.sessionId, targets);
      startTicker();
      return null;
    },
  };

  /** Commands answer after a latency like a LAN server; listing big folders takes longer. */
  const slow: Record<string, number> = {
    connect: 5,
    test_connection: 5,
    reconnect: 4,
    list_dir: 1.2,
    open_file: 1,
    save_file: 1.5,
  };

  async function invoke(command: string, args: Record<string, unknown>): Promise<unknown> {
    const handler = commands[command];
    if (!handler)
      throw new MockError('internal', `The browser preview has no "${command}" command.`);
    await wait(slow[command] ?? 0.4);
    try {
      return handler(args);
    } catch (error) {
      if (error instanceof MockError) {
        // The UI receives plain AppError objects, as from Rust.
        throw { code: error.code, message: error.message, hostKey: error.hostKey };
      }
      throw error;
    }
  }

  /** Console and test helpers: things that happen "on the server" or "on the network". */
  const controls = {
    /** Changes a file on the server behind the UI's back (the next save conflicts). */
    changeOnServer(path: string, line = '# changed on the server\n') {
      const node = tree.get(path);
      if (node?.kind !== 'file') return false;
      node.text += line;
      node.size = storedSize(node.text, node.encoding, node.lineEnding, node.mixed);
      node.mtime += 60;
      return true;
    },
    /** Drops every session (or one): the tabs show "Disconnected · Reconnect". */
    dropConnection(sessionId?: string) {
      for (const session of sessions.values()) {
        if (sessionId && session.info.id !== sessionId) continue;
        session.dropped = true;
        shell.failAll(session.info.id, 'The connection was lost.');
        write(session, 'error', 'Connection reset by peer');
        session.sink({ type: 'state', state: 'disconnected', reason: 'Connection reset by peer' });
      }
    },
    /** Emulates files dragged from the OS and dropped at (x, y) in the window. */
    dropFiles(names: string[], x: number, y: number) {
      const dropId = newId('drop');
      drops.set(dropId, names);
      options.emit?.('gem://drag', { kind: 'over', x, y });
      setTimeout(() => options.emit?.('gem://drop', { dropId, names, x, y }), 400);
      return dropId;
    },
    setLatency(ms: number) {
      latency = ms;
    },
    reset() {
      tree = new MockTree();
      jobs.clear();
    },
    get tree() {
      return tree;
    },
    get sessions() {
      return [...sessions.values()].map((session) => session.info);
    },
    knownHosts,
    bigLogSize: BIG_LOG_SIZE,
  };

  return { invoke, controls };
}

export type MockBackend = ReturnType<typeof createMockBackend>;
export type MockControls = MockBackend['controls'];
