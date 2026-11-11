import { spawnSync } from 'node:child_process';
import { copyFileSync, existsSync, mkdtempSync, rmSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { createRequire } from 'node:module';
import { fileURLToPath } from 'node:url';

const project = fileURLToPath(new URL('../', import.meta.url));
const require = createRequire(import.meta.url);
let tauriCli;
try {
  tauriCli = require.resolve('@tauri-apps/cli/tauri.js');
} catch {
  throw new Error(
    'The Tauri CLI is missing. Run pnpm install --frozen-lockfile, then retry pnpm icons.',
  );
}

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: project,
    stdio: ['ignore', 'inherit', 'inherit'],
  });

  if (result.error) throw result.error;
  if (result.status !== 0)
    throw new Error(`${command} failed with exit code ${result.status ?? 1}.`);
}

if (process.platform !== 'darwin') {
  throw new Error(
    'Rebuilding icons from AppIcon.icon requires macOS and Xcode with Icon Composer.',
  );
}

const selectedXcode = spawnSync('xcode-select', ['-p'], { encoding: 'utf8' });
if (selectedXcode.error) throw selectedXcode.error;
if (selectedXcode.status !== 0)
  throw new Error('Select Xcode with xcode-select before rebuilding icons.');

const renderer = join(
  selectedXcode.stdout.trim(),
  '../Applications/Icon Composer.app/Contents/Executables/ictool',
);

if (!existsSync(renderer)) {
  throw new Error('The selected Xcode does not include the Icon Composer image exporter (ictool).');
}

const temporaryDirectory = mkdtempSync(join(tmpdir(), 'gem-icons-'));
const exportedIcon = join(temporaryDirectory, 'AppIcon.png');

try {
  run(renderer, [
    join(project, 'src-tauri/icons/AppIcon.icon'),
    '--export-image',
    '--output-file',
    exportedIcon,
    '--platform',
    'macOS',
    '--rendition',
    'Default',
    '--width',
    '1024',
    '--height',
    '1024',
    '--scale',
    '1',
  ]);

  // Only the macOS icon keeps the Icon Composer look; other platforms use the flat tile.
  const macOutput = join(temporaryDirectory, 'mac');
  run(process.execPath, [tauriCli, 'icon', exportedIcon, '--output', macOutput]);
  copyFileSync(join(macOutput, 'icon.icns'), join(project, 'src-tauri/icons/icon.icns'));
  run(process.execPath, ['scripts/build-flat-icons.mjs']);
  run(process.execPath, ['scripts/build-macos-icon.mjs']);
} finally {
  rmSync(temporaryDirectory, { recursive: true, force: true });
}
