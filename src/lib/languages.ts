/**
 * Language detection for the viewer (plan §2.3): exact file name → path rules → extension →
 * first line (shebang, `<?php`, `<?xml`) → plain text. One table, tested in
 * `languages.test.ts`. The CodeMirror packages are loaded lazily by `languageSupport.ts`.
 */

export const LANGUAGE_IDS = [
  'apache',
  'css',
  'dockerfile',
  'dotenv',
  'go',
  'html',
  'ini',
  'javascript',
  'json',
  'jsx',
  'log',
  'makefile',
  'markdown',
  'nginx',
  'php',
  'properties',
  'python',
  'rust',
  'scss',
  'shell',
  'sql',
  'text',
  'toml',
  'tsx',
  'typescript',
  'xml',
  'yaml',
] as const;

export type LanguageId = (typeof LANGUAGE_IDS)[number];

export const LANGUAGE_LABEL: Record<LanguageId, string> = {
  apache: 'Apache conf',
  css: 'CSS',
  dockerfile: 'Dockerfile',
  dotenv: '.env',
  go: 'Go',
  html: 'HTML',
  ini: 'INI',
  javascript: 'JavaScript',
  json: 'JSON',
  jsx: 'JavaScript (JSX)',
  log: 'Log',
  makefile: 'Makefile',
  markdown: 'Markdown',
  nginx: 'nginx',
  php: 'PHP',
  properties: 'Properties',
  python: 'Python',
  rust: 'Rust',
  scss: 'SCSS',
  shell: 'Shell',
  sql: 'SQL',
  text: 'Plain text',
  toml: 'TOML',
  tsx: 'TypeScript (TSX)',
  typescript: 'TypeScript',
  xml: 'XML',
  yaml: 'YAML',
};

/** Picker order: alphabetical by label, plain text last. */
export const LANGUAGE_OPTIONS: { id: LanguageId; label: string }[] = [
  ...LANGUAGE_IDS.filter((id) => id !== 'text')
    .map((id): { id: LanguageId; label: string } => ({ id, label: LANGUAGE_LABEL[id] }))
    .sort((a, b) => a.label.localeCompare(b.label, 'en', { sensitivity: 'base' })),
  { id: 'text', label: LANGUAGE_LABEL.text },
];

/** Exact file names (case-sensitive first, then lower case). */
const FILE_NAMES: Record<string, LanguageId> = {
  Dockerfile: 'dockerfile',
  Containerfile: 'dockerfile',
  dockerfile: 'dockerfile',
  Makefile: 'makefile',
  makefile: 'makefile',
  GNUmakefile: 'makefile',
  '.env': 'dotenv',
  'nginx.conf': 'nginx',
  fastcgi_params: 'nginx',
  'fastcgi.conf': 'nginx',
  proxy_params: 'nginx',
  uwsgi_params: 'nginx',
  scgi_params: 'nginx',
  'mime.types': 'nginx',
  '.htaccess': 'apache',
  '.htpasswd': 'text',
  'httpd.conf': 'apache',
  'apache2.conf': 'apache',
  'ports.conf': 'apache',
  'composer.lock': 'json',
  '.babelrc': 'json',
  '.eslintrc': 'json',
  '.prettierrc': 'json',
  '.jshintrc': 'json',
  '.bashrc': 'shell',
  '.bash_profile': 'shell',
  '.bash_login': 'shell',
  '.bash_logout': 'shell',
  '.bash_aliases': 'shell',
  '.profile': 'shell',
  '.zshrc': 'shell',
  '.zprofile': 'shell',
  '.zshenv': 'shell',
  '.kshrc': 'shell',
  PKGBUILD: 'shell',
  'Cargo.lock': 'toml',
  Pipfile: 'toml',
  'poetry.lock': 'toml',
  '.editorconfig': 'ini',
  '.gitconfig': 'ini',
  '.npmrc': 'ini',
  '.pylintrc': 'ini',
  'my.cnf': 'ini',
  'php-fpm.conf': 'ini',
  'www.conf': 'ini',
  'supervisord.conf': 'ini',
  'requirements.txt': 'text',
  syslog: 'log',
  messages: 'log',
  access_log: 'log',
  error_log: 'log',
};

const EXTENSIONS: Record<string, LanguageId> = {
  php: 'php',
  phtml: 'php',
  php3: 'php',
  php4: 'php',
  php5: 'php',
  php7: 'php',
  php8: 'php',
  phps: 'php',
  go: 'go',
  yaml: 'yaml',
  yml: 'yaml',
  json: 'json',
  jsonc: 'json',
  json5: 'json',
  webmanifest: 'json',
  har: 'json',
  ipynb: 'json',
  js: 'javascript',
  mjs: 'javascript',
  cjs: 'javascript',
  jsx: 'jsx',
  ts: 'typescript',
  mts: 'typescript',
  cts: 'typescript',
  tsx: 'tsx',
  html: 'html',
  htm: 'html',
  xhtml: 'html',
  shtml: 'html',
  twig: 'html',
  vue: 'html',
  svelte: 'html',
  css: 'css',
  less: 'css',
  scss: 'scss',
  sass: 'scss',
  xml: 'xml',
  xsd: 'xml',
  xsl: 'xml',
  xslt: 'xml',
  svg: 'xml',
  plist: 'xml',
  rss: 'xml',
  atom: 'xml',
  wsdl: 'xml',
  xaml: 'xml',
  csproj: 'xml',
  xlf: 'xml',
  sql: 'sql',
  psql: 'sql',
  mysql: 'sql',
  pgsql: 'sql',
  py: 'python',
  pyw: 'python',
  pyi: 'python',
  wsgi: 'python',
  rs: 'rust',
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  ksh: 'shell',
  ash: 'shell',
  dash: 'shell',
  fish: 'shell',
  command: 'shell',
  md: 'markdown',
  markdown: 'markdown',
  mdx: 'markdown',
  mkd: 'markdown',
  ini: 'ini',
  cfg: 'ini',
  cnf: 'ini',
  conf: 'ini',
  inf: 'ini',
  desktop: 'ini',
  service: 'ini',
  timer: 'ini',
  socket: 'ini',
  properties: 'properties',
  env: 'dotenv',
  toml: 'toml',
  log: 'log',
  out: 'log',
  txt: 'text',
  text: 'text',
  dockerfile: 'dockerfile',
  containerfile: 'dockerfile',
  mk: 'makefile',
  mak: 'makefile',
  make: 'makefile',
  htaccess: 'apache',
  vhost: 'apache',
  nginx: 'nginx',
};

/** Interpreters named by a shebang (version suffixes are stripped). */
const INTERPRETERS: Record<string, LanguageId> = {
  sh: 'shell',
  bash: 'shell',
  zsh: 'shell',
  ksh: 'shell',
  dash: 'shell',
  ash: 'shell',
  fish: 'shell',
  python: 'python',
  pypy: 'python',
  node: 'javascript',
  nodejs: 'javascript',
  deno: 'typescript',
  bun: 'javascript',
  'ts-node': 'typescript',
  tsx: 'typescript',
  php: 'php',
  make: 'makefile',
};

function fromPath(path: string, name: string, extension: string): LanguageId | null {
  const dirs = path.split('/').slice(0, -1);
  const inDir = (...names: string[]) => dirs.some((dir) => names.includes(dir));
  // Server configuration lives in well-known folders, often in files without an extension
  // (`/etc/nginx/sites-available/default`).
  if (
    inDir('nginx') &&
    (extension === '' ||
      extension === 'conf' ||
      inDir('sites-available', 'sites-enabled', 'conf.d', 'snippets'))
  ) {
    return 'nginx';
  }
  if (inDir('apache2', 'httpd', 'apache') && (extension === '' || extension === 'conf')) {
    return 'apache';
  }
  if (/^\.env(\.|$)/.test(name)) return 'dotenv';
  if (/^(Docker|Container)file\./.test(name)) return 'dockerfile';
  if (/\.log(\.\d+)?$/.test(name)) return 'log';
  if (inDir('log', 'logs') && extension === '') return 'log';
  return null;
}

function fromFirstLine(line: string): LanguageId | null {
  const first = line.replace(/^\uFEFF/, '').trimEnd();
  const shebang = /^#!\s*(\S+)(?:\s+(.*))?$/.exec(first);
  if (shebang) {
    let program = shebang[1]!.split('/').pop() ?? '';
    if (program === 'env') {
      // `#!/usr/bin/env -S node --flags` → node
      const args = (shebang[2] ?? '').split(/\s+/).filter((arg) => arg && !arg.startsWith('-'));
      program = args[0] ?? '';
    }
    const base = program.replace(/[\d.]+$/, '');
    return INTERPRETERS[base] ?? null;
  }
  if (/^<\?php\b/i.test(first) || first === '<?') return 'php';
  if (/^<\?xml\b/i.test(first)) return 'xml';
  if (/^<!doctype html\b/i.test(first) || /^<html[\s>]/i.test(first)) return 'html';
  return null;
}

/**
 * The language of a remote file from its path and, when known, its first line.
 * `path` may be a bare name.
 */
export function detectLanguage(path: string, firstLine = ''): LanguageId {
  const name = path.slice(path.lastIndexOf('/') + 1);
  const exact = FILE_NAMES[name] ?? FILE_NAMES[name.toLowerCase()];
  if (exact) return exact;

  const dot = name.lastIndexOf('.');
  // A leading dot alone (`.bashrc`) is not an extension.
  const extension = dot > 0 ? name.slice(dot + 1).toLowerCase() : '';

  const byPath = fromPath(path, name, extension);
  if (byPath) return byPath;

  const byExtension = extension ? EXTENSIONS[extension] : undefined;
  if (byExtension && byExtension !== 'text') return byExtension;

  return fromFirstLine(firstLine) ?? byExtension ?? 'text';
}

/** The first line of a text, cheaply (the viewer may hold megabytes). */
export function firstLineOf(text: string): string {
  const end = text.indexOf('\n');
  return end < 0 ? text.slice(0, 500) : text.slice(0, Math.min(end, 500));
}

export function isLanguageId(value: string): value is LanguageId {
  return (LANGUAGE_IDS as readonly string[]).includes(value);
}
