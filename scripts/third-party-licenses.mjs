#!/usr/bin/env node
// Generates src-tauri/resources/THIRD_PARTY_LICENSES.md: every production npm package bundled
// into the UI and every Rust crate linked into the binary (all targets, proc-macros excluded),
// with the licence texts found in each package. Shipped with the app (bundle.resources) to
// satisfy MIT/BSD/ISC/Apache attribution requirements. Run automatically before `tauri build`.
import { execFileSync } from 'node:child_process';
import { existsSync, mkdirSync, readdirSync, readFileSync, writeFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const out = join(root, 'src-tauri', 'resources', 'THIRD_PARTY_LICENSES.md');
const LICENSE_FILE = /^(licen[cs]e|copying|notice)([.-].*)?$/i;

function run(cmd, args, cwd = root) {
  return execFileSync(cmd, args, { cwd, encoding: 'utf8', maxBuffer: 256 * 1024 * 1024 });
}

function licenseTexts(dir) {
  if (!dir || !existsSync(dir)) return [];
  return readdirSync(dir)
    .filter((name) => LICENSE_FILE.test(name))
    .sort()
    .map((name) => ({ name, text: readFileSync(join(dir, name), 'utf8').trim() }));
}

// npm: production dependencies only (what ends up in the bundled UI).
const npm = [];
const byLicense = JSON.parse(run('pnpm', ['licenses', 'list', '--prod', '--json']));
for (const [license, packages] of Object.entries(byLicense)) {
  for (const pkg of packages) {
    pkg.versions.forEach((version, i) =>
      npm.push({ name: pkg.name, version, license, texts: licenseTexts(pkg.paths[i]) }),
    );
  }
}

// Rust: crates linked into the binary on any target platform.
const tauriDir = join(root, 'src-tauri');
const metadata = JSON.parse(
  run('cargo', ['metadata', '--format-version', '1', '--locked'], tauriDir),
);
const dirs = new Map(
  metadata.packages.map((p) => [`${p.name} ${p.version}`, dirname(p.manifest_path)]),
);
const linked = new Map();
for (const line of run(
  'cargo',
  ['tree', '-e', 'normal', '--target', 'all', '--prefix', 'none', '-f', '{p}|{l}'],
  tauriDir,
).split('\n')) {
  if (!line.trim() || line.includes('(proc-macro)')) continue;
  const [id, license = 'UNKNOWN'] = line.replace(' (*)', '').split('|');
  const [name, version] = id.trim().split(' ');
  if (name === 'gem') continue;
  const key = `${name} ${version.replace(/^v/, '')}`;
  linked.set(key, {
    name,
    version: version.replace(/^v/, ''),
    license: license.trim() || 'UNKNOWN',
    texts: licenseTexts(dirs.get(key)),
  });
}
const rust = [...linked.values()];

const sort = (a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version);

// Identical licence texts (the same MIT/Apache text in hundreds of crates) are printed once.
const texts = new Map();
const refOf = (text) => {
  const normal = text.replace(/\r\n/g, '\n').trim();
  if (!texts.has(normal)) texts.set(normal, texts.size + 1);
  return texts.get(normal);
};
const table = (title, items) =>
  [
    `## ${title} (${items.length})`,
    '',
    '| Package | Version | Licence | Text |',
    '|---|---|---|---|',
  ]
    .concat(
      items.sort(sort).map((item) => {
        const refs = item.texts.map((t) => `[${refOf(t.text)}](#text-${refOf(t.text)})`).join(' ');
        return `| ${item.name} | ${item.version} | ${item.license} | ${refs || '—'} |`;
      }),
    )
    .join('\n');

mkdirSync(dirname(out), { recursive: true });
writeFileSync(
  out,
  [
    '# Third-party software in Gem',
    '',
    'Gem is MIT-licensed. It includes the following third-party software, under their own licences.',
    '',
    table('JavaScript packages (user interface)', npm),
    '',
    table('Rust crates (application binary)', rust),
    '',
    '## Licence texts',
    '',
    'Packages marked "—" ship no licence file; their licence is the SPDX expression shown.',
    '',
    ...[...texts.entries()].flatMap(([text, n]) => [
      `### Text ${n}`,
      `<a id="text-${n}"></a>`,
      '',
      '```text',
      text.replace(/```/g, "'''"),
      '```',
      '',
    ]),
  ].join('\n'),
);
console.log(`wrote ${out}: ${npm.length} npm packages, ${rust.length} crates`);
