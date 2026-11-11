import { spawnSync } from 'node:child_process';
import { cpSync, mkdtempSync, readdirSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

// Linux, Windows and Android use the flat tile artwork, like the other co-op apps.
// icon.icns is left alone: it comes from the Icon Composer render (see build-icons.mjs).
const project = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(import.meta.url);
let tauriCli;
try {
  tauriCli = require.resolve('@tauri-apps/cli/tauri.js');
} catch {
  throw new Error(
    'The Tauri CLI is missing. Run pnpm install --frozen-lockfile, then retry pnpm icons:png.',
  );
}

const source = join(project, 'src/assets/AppIcon.png');
const icons = join(project, 'src-tauri/icons');
const skipped = new Set(['icon.icns', 'ios']);
const temporaryDirectory = mkdtempSync(join(tmpdir(), 'gem-flat-icons-'));

try {
  const result = spawnSync(
    process.execPath,
    [tauriCli, 'icon', source, '--output', temporaryDirectory],
    {
      cwd: project,
      stdio: ['ignore', 'inherit', 'inherit'],
    },
  );
  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`tauri icon failed with exit code ${result.status ?? 1}.`);

  for (const entry of readdirSync(temporaryDirectory)) {
    if (skipped.has(entry)) continue;
    cpSync(join(temporaryDirectory, entry), join(icons, entry), { recursive: true });
  }
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}
