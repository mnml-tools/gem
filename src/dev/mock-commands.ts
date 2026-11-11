/**
 * Dev mock of remote commands and snippets (ADR 0014): snippets in memory (globals and one
 * site's), and command runs that stream realistic output through their channel, then exit.
 * `tail -f`-style commands stream until `cancel_command`. Synthetic data only.
 */
import type { CommandEvent } from '@/lib/ipc/bindings/CommandEvent';
import type { CommandStream } from '@/lib/ipc/bindings/CommandStream';
import type { Snippet } from '@/lib/ipc/bindings/Snippet';
import type { SnippetInput } from '@/lib/ipc/bindings/SnippetInput';
import { joinPath } from '@/lib/paths';

type Sink = (event: CommandEvent) => void;

type Run = {
  id: string;
  sessionId: string;
  sink: Sink;
  timers: ReturnType<typeof setTimeout>[];
  done: boolean;
  started: number;
};

export type CommandHost = {
  /** Base delay between output chunks, in ms (0 in tests). */
  delay: () => number;
  /** Lists a folder of the fake tree for `ls`: `[name, isDir]`. */
  list: (dir: string) => [string, boolean][] | null;
  log: (sessionId: string, kind: 'command' | 'status' | 'error', text: string) => void;
  /** `touch`: creates an empty file if missing; false when its folder does not exist. */
  touch: (path: string) => boolean;
  /** `rm`: removes a file; false when there is none. */
  remove: (path: string) => boolean;
};

const stamp = () => new Date(Date.now() - 20 * 86_400_000).toISOString();

function seed(mode: 'demo' | 'empty'): Snippet[] {
  if (mode === 'empty') return [];
  const base = { createdAt: stamp(), updatedAt: stamp() };
  const globals: [string, string, boolean][] = [
    ['Crontab', 'crontab -l', false],
    ['Containers', 'docker ps', false],
    ['Core logs', 'docker logs core | tail -30', false],
    ['Disk usage', 'df -h', false],
    ['Uptime', 'uptime', false],
    ['Folder size', 'du -sh {dir}', false],
  ];
  const snippets: Snippet[] = globals.map(([name, command, confirm], index) => ({
    ...base,
    id: `snippet-g${index}`,
    siteId: null,
    name,
    command,
    confirm,
    sortOrder: index,
  }));
  snippets.push(
    {
      ...base,
      id: 'snippet-restart',
      siteId: 'site-prod',
      name: 'Restart core',
      command: 'docker restart core',
      confirm: true,
      sortOrder: 0,
    },
    {
      ...base,
      id: 'snippet-follow',
      siteId: 'site-prod',
      name: 'Follow app log',
      command: 'tail -f storage/logs/laravel.log',
      confirm: false,
      sortOrder: 1,
    },
  );
  return snippets;
}

const quote = (value: string) => `'${value.replace(/'/g, `'\\''`)}'`;

function clock(offsetSeconds: number): string {
  const date = new Date(Date.now() - offsetSeconds * 1000);
  return `${date.toISOString().slice(0, 19)}Z`;
}

/** Output of a finished command: `[stream, text]` chunks and the exit code. */
function script(
  command: string,
  host: CommandHost,
  dir: string | null,
): { chunks: [CommandStream, string][]; code: number } | 'follow' {
  const trimmed = command.trim();
  const lines = (text: string): [CommandStream, string][] =>
    text.split(/(?<=\n)/).map((line) => ['stdout', line]);
  if (/\btail\s+-f\b|\blogs\s+-f\b|--follow\b/.test(trimmed)) return 'follow';
  if (trimmed === 'crontab -l') {
    return {
      code: 0,
      chunks: lines(
        '# m h  dom mon dow   command\n' +
          '*/5 * * * * cd /var/www/api && php artisan schedule:run >> /dev/null 2>&1\n' +
          '15 3 * * * /var/www/api/scripts/backup.py --quiet\n' +
          '0 4 * * 0 /var/www/api/scripts/rotate /var/www/api/storage/logs 14\n',
      ),
    };
  }
  if (trimmed === 'docker ps') {
    return {
      code: 0,
      chunks: lines(
        'CONTAINER ID   IMAGE                    COMMAND                  STATUS          PORTS                    NAMES\n' +
          '3f2a9c1d7e44   acme/api:2026.11.07-3    "docker-php-entrypoi…"   Up 2 days       9000/tcp                 core\n' +
          '8b1e0d55a3c2   nginx:1.27-alpine        "/docker-entrypoint.…"   Up 2 days       0.0.0.0:443->443/tcp     web\n' +
          'c7d4e2f90a11   mysql:8.4                "docker-entrypoint.s…"   Up 9 days       3306/tcp                 db\n' +
          '1a9f3b7c0e22   redis:7.4-alpine         "docker-entrypoint.s…"   Up 9 days       6379/tcp                 cache\n',
      ),
    };
  }
  if (/^docker logs core/.test(trimmed)) {
    const chunks: [CommandStream, string][] = [];
    for (let index = 30; index > 0; index -= 1) {
      const at = clock(index * 7);
      if (index === 9) {
        chunks.push([
          'stderr',
          `${at} ERROR [queue] job BuildExport id=4471 failed: SQLSTATE[HY000] [2002] Connection refused\n`,
        ]);
        continue;
      }
      const level = index % 6 === 0 ? 'WARN ' : 'INFO ';
      const route = ['GET /api/users', 'POST /api/invoices', 'GET /healthz', 'GET /api/search'][
        index % 4
      ];
      chunks.push([
        'stdout',
        `${at} ${level} [http] ${route} ${index % 6 === 0 ? 429 : 200} ${10 + (index % 7) * 13}ms\n`,
      ]);
    }
    return { code: 0, chunks };
  }
  if (trimmed === 'df -h') {
    return {
      code: 0,
      chunks: lines(
        'Filesystem      Size  Used Avail Use% Mounted on\n' +
          '/dev/vda1        79G   51G   25G  68% /\n' +
          'tmpfs           3.9G     0  3.9G   0% /dev/shm\n' +
          '/dev/vdb1       197G  143G   45G  77% /var/lib/docker\n',
      ),
    };
  }
  if (trimmed === 'uptime') {
    return {
      code: 0,
      chunks: lines(' 10:42:01 up 37 days,  4:12,  1 user,  load average: 0.42, 0.35, 0.31\n'),
    };
  }
  if (trimmed === 'docker restart core') return { code: 0, chunks: lines('core\n') };
  const du = /^du -sh (.+)$/.exec(trimmed);
  if (du) return { code: 0, chunks: lines(`184M\t${du[1]!.replace(/^'|'$/g, '')}\n`) };
  const ls = /^ls(?:\s+-\w+)*\s*(.*)$/.exec(trimmed);
  if (ls) {
    const dir = (ls[1] ?? '').replace(/^'|'$/g, '') || '.';
    const entries = host.list(dir);
    if (!entries) {
      return {
        code: 2,
        chunks: [['stderr', `ls: cannot access '${dir}': No such file or directory\n`]],
      };
    }
    return {
      code: 0,
      chunks: lines(
        entries
          .map(
            ([name, folder]) =>
              `${folder ? 'drwxr-xr-x' : '-rw-r--r--'}  ${name}${folder ? '/' : ''}\n`,
          )
          .join(''),
      ),
    };
  }
  const change = /^(touch|rm(?: -f)?)\s+(.+)$/.exec(trimmed);
  if (change) {
    const touch = change[1] === 'touch';
    // Quoted parts glue to their neighbours ('/srv/app'/new.txt); no spaces inside names.
    const paths = change[2]!.split(/\s+/).map((arg) => {
      const path = arg.replace(/'/g, '');
      return path.startsWith('/') ? path : joinPath(dir ?? '/', path);
    });
    const errors = paths
      .filter((path) => !(touch ? host.touch(path) : host.remove(path)))
      .map((path) =>
        touch
          ? `touch: cannot touch '${path}': No such file or directory\n`
          : `rm: cannot remove '${path}': No such file or directory\n`,
      );
    return { code: errors.length ? 1 : 0, chunks: errors.map((text) => ['stderr', text]) };
  }
  if (/^(false|exit 1|test -f .*)$/.test(trimmed) || trimmed === 'systemctl is-active worker') {
    return { code: 1, chunks: trimmed.startsWith('systemctl') ? lines('inactive\n') : [] };
  }
  if (/^echo\b/.test(trimmed))
    return { code: 0, chunks: lines(`${trimmed.slice(5).replace(/^['"]|['"]$/g, '')}\n`) };
  const program = trimmed.split(/\s+/)[0] ?? '';
  return { code: 127, chunks: [['stderr', `bash: ${program}: command not found\n`]] };
}

export function createCommandMock(mode: 'demo' | 'empty', host: CommandHost) {
  let snippets = seed(mode);
  const runs = new Map<string, Run>();
  let counter = 0;

  const ordered = () =>
    [...snippets].sort(
      (a, b) =>
        (a.siteId === null ? 0 : 1) - (b.siteId === null ? 0 : 1) || a.sortOrder - b.sortOrder,
    );

  const finish = (run: Run, event: CommandEvent) => {
    if (run.done) return;
    run.done = true;
    run.timers.forEach(clearTimeout);
    run.sink(event);
    runs.delete(run.id);
  };

  return {
    listSnippets(siteId: string | null): Snippet[] {
      return ordered().filter((item) => item.siteId === null || item.siteId === siteId);
    },
    createSnippet(input: SnippetInput): Snippet {
      const now = new Date().toISOString();
      const snippet: Snippet = {
        ...input,
        id: `snippet-${(counter += 1).toString(36)}-${Date.now().toString(36)}`,
        sortOrder: snippets.filter((item) => item.siteId === input.siteId).length,
        createdAt: now,
        updatedAt: now,
      };
      snippets = [...snippets, snippet];
      return snippet;
    },
    updateSnippet(id: string, input: SnippetInput): Snippet | null {
      const current = snippets.find((item) => item.id === id);
      if (!current) return null;
      const moved = current.siteId !== input.siteId;
      const updated: Snippet = {
        ...current,
        ...input,
        sortOrder: moved
          ? snippets.filter((item) => item.siteId === input.siteId).length
          : current.sortOrder,
        updatedAt: new Date().toISOString(),
      };
      snippets = snippets.map((item) => (item.id === id ? updated : item));
      return updated;
    },
    deleteSnippet(id: string): void {
      snippets = snippets.filter((item) => item.id !== id);
    },
    deleteSiteSnippets(siteId: string): void {
      snippets = snippets.filter((item) => item.siteId !== siteId);
    },
    reorderSnippets(ids: string[]): void {
      snippets = snippets.map((item) =>
        ids.includes(item.id) ? { ...item, sortOrder: ids.indexOf(item.id) } : item,
      );
    },

    /** Starts a run; returns its id and the expanded command. */
    run(sessionId: string, command: string, dir: string | null, sink: Sink) {
      const expanded = command.split('{dir}').join(quote(dir ?? '.'));
      const run: Run = {
        id: `run-${(counter += 1).toString(36)}`,
        sessionId,
        sink,
        timers: [],
        done: false,
        started: Date.now(),
      };
      runs.set(run.id, run);
      host.log(sessionId, 'command', `exec ${expanded}`);
      const delay = host.delay();
      const at = (ms: number, action: () => void) => {
        run.timers.push(setTimeout(() => !run.done && action(), ms));
      };
      const out = (stream: CommandStream, text: string) => sink({ type: 'output', stream, text });
      const plan = script(expanded, host, dir);
      if (plan === 'follow') {
        let line = 0;
        const tick = () => {
          if (run.done) return;
          line += 1;
          const level = line % 11 === 0 ? 'ERROR' : line % 5 === 0 ? 'WARNING' : 'INFO';
          const text = `[${clock(0).replace('T', ' ').slice(0, 19)}] production.${level}: ${
            level === 'ERROR'
              ? 'Payment webhook signature mismatch'
              : `Request handled in ${12 + (line % 9) * 7}ms`
          } {"req":"${((line * 2654435761) >>> 0).toString(16).slice(0, 8)}"}\n`;
          out(level === 'ERROR' ? 'stderr' : 'stdout', text);
          at(delay ? 400 : 5, tick);
        };
        at(delay ? 150 : 0, tick);
      } else {
        let time = delay ? 120 : 0;
        for (const [stream, text] of plan.chunks) {
          at(time, () => out(stream, text));
          time += delay ? 25 + (text.length % 40) : 0;
        }
        at(time + (delay ? 80 : 0), () => {
          host.log(
            sessionId,
            plan.code === 0 ? 'status' : 'error',
            `exit ${plan.code}: ${expanded}`,
          );
          finish(run, {
            type: 'exit',
            exit: { code: plan.code, signal: null, cancelled: false },
            durationMs: Date.now() - run.started,
          });
        });
      }
      return { runId: run.id, command: expanded };
    },

    cancel(runId: string): void {
      const run = runs.get(runId);
      if (!run) return;
      finish(run, {
        type: 'exit',
        exit: { code: null, signal: 'TERM', cancelled: true },
        durationMs: Date.now() - run.started,
      });
    },

    /** The connection dropped: every run of it fails. */
    failAll(sessionId: string, message: string): void {
      for (const run of [...runs.values()]) {
        if (run.sessionId !== sessionId) continue;
        finish(run, { type: 'failed', error: { code: 'disconnected', message, hostKey: null } });
      }
    },
  };
}
