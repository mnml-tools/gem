import { spawnSync } from 'node:child_process';
import { mkdirSync, rmSync, statSync } from 'node:fs';
import { fileURLToPath } from 'node:url';

const project = new URL('../', import.meta.url);
const source = fileURLToPath(new URL('src-tauri/icons/AppIcon.icon', project));
const output = fileURLToPath(new URL('src-tauri/target/macos-icon/', project));

if (process.platform !== 'darwin') {
  throw new Error('The macOS icon must be compiled on macOS with Xcode installed.');
}

const xcode = spawnSync('xcodebuild', ['-version'], { encoding: 'utf8' });
if (xcode.error) throw xcode.error;
const majorVersion = Number(xcode.stdout?.match(/^Xcode (\d+)/m)?.[1]);
if (xcode.status !== 0 || !Number.isFinite(majorVersion) || majorVersion < 26) {
  throw new Error(
    'Compiling AppIcon.icon requires Xcode 26 or newer. Select it with xcode-select or DEVELOPER_DIR.',
  );
}

mkdirSync(output, { recursive: true });
const catalog = `${output}/Assets.car`;
// A cached catalog must not hide a compiler that produces no output.
rmSync(catalog, { force: true });

// Xcode 27's icon compiler can fail with inherited stdin when launched by Tauri.
// Compile separately with stdin closed, then let Tauri package the asset catalog.
const result = spawnSync(
  'xcrun',
  [
    'actool',
    source,
    '--compile',
    output,
    '--output-format',
    'human-readable-text',
    '--output-partial-info-plist',
    `${output}/icon-info.plist`,
    '--app-icon',
    'AppIcon',
    '--include-all-app-icons',
    '--platform',
    'macosx',
    '--target-device',
    'mac',
    '--minimum-deployment-target',
    '26.0',
  ],
  { stdio: ['ignore', 'inherit', 'inherit'] },
);

if (result.error) throw result.error;
if (result.status !== 0) process.exit(result.status ?? 1);
if (!statSync(catalog, { throwIfNoEntry: false })?.size) {
  throw new Error(
    `actool did not generate ${catalog}. Check the selected Xcode supports AppIcon.icon.`,
  );
}
