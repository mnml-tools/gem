import { describe, expect, it } from 'vitest';
import {
  detectLanguage,
  firstLineOf,
  isLanguageId,
  LANGUAGE_IDS,
  LANGUAGE_LABEL,
  LANGUAGE_OPTIONS,
} from './languages';

describe('detectLanguage: exact file names', () => {
  it.each([
    ['Dockerfile', 'dockerfile'],
    ['Containerfile', 'dockerfile'],
    ['/srv/app/Dockerfile', 'dockerfile'],
    ['Dockerfile.prod', 'dockerfile'],
    ['Makefile', 'makefile'],
    ['GNUmakefile', 'makefile'],
    ['.env', 'dotenv'],
    ['.env.example', 'dotenv'],
    ['.env.production.local', 'dotenv'],
    ['nginx.conf', 'nginx'],
    ['/etc/nginx/fastcgi_params', 'nginx'],
    ['.htaccess', 'apache'],
    ['httpd.conf', 'apache'],
    ['apache2.conf', 'apache'],
    ['composer.lock', 'json'],
    ['.babelrc', 'json'],
    ['.bashrc', 'shell'],
    ['.profile', 'shell'],
    ['.zshrc', 'shell'],
    ['Cargo.lock', 'toml'],
    ['Pipfile', 'toml'],
    ['.editorconfig', 'ini'],
    ['.gitconfig', 'ini'],
    ['php-fpm.conf', 'ini'],
  ] as const)('%s → %s', (path, language) => {
    expect(detectLanguage(path)).toBe(language);
  });
});

describe('detectLanguage: server configuration folders', () => {
  it('reads files without an extension under nginx as nginx (CA-01)', () => {
    expect(detectLanguage('/etc/nginx/sites-available/default')).toBe('nginx');
    expect(detectLanguage('/etc/nginx/sites-enabled/example.com')).toBe('nginx');
    expect(detectLanguage('/etc/nginx/conf.d/gzip.conf')).toBe('nginx');
    expect(detectLanguage('/etc/nginx/snippets/ssl.conf')).toBe('nginx');
  });

  it('reads Apache sites and conf files as Apache conf', () => {
    expect(detectLanguage('/etc/apache2/sites-available/000-default.conf')).toBe('apache');
    expect(detectLanguage('/etc/httpd/conf/httpd.conf')).toBe('apache');
    expect(detectLanguage('/etc/apache2/envvars')).toBe('apache');
  });

  it('keeps other extensions inside those folders', () => {
    expect(detectLanguage('/etc/nginx/html/index.html')).toBe('html');
    expect(detectLanguage('/etc/apache2/magic.yaml')).toBe('yaml');
  });

  it('reads extensionless files under log folders as logs', () => {
    expect(detectLanguage('/var/log/syslog')).toBe('log');
    expect(detectLanguage('/var/log/nginx/access')).toBe('nginx');
    expect(detectLanguage('/srv/app/logs/worker')).toBe('log');
  });
});

describe('detectLanguage: extensions', () => {
  it.each([
    ['index.php', 'php'],
    ['view.phtml', 'php'],
    ['main.go', 'go'],
    ['docker-compose.yml', 'yaml'],
    ['config.yaml', 'yaml'],
    ['composer.json', 'json'],
    ['app.js', 'javascript'],
    ['server.mjs', 'javascript'],
    ['component.jsx', 'jsx'],
    ['server.ts', 'typescript'],
    ['App.tsx', 'tsx'],
    ['index.html', 'html'],
    ['page.htm', 'html'],
    ['app.css', 'css'],
    ['theme.scss', 'scss'],
    ['sitemap.xml', 'xml'],
    ['logo.svg', 'xml'],
    ['schema.sql', 'sql'],
    ['backup.py', 'python'],
    ['main.rs', 'rust'],
    ['deploy.sh', 'shell'],
    ['setup.bash', 'shell'],
    ['README.md', 'markdown'],
    ['app.ini', 'ini'],
    ['supervisor.conf', 'ini'],
    ['settings.properties', 'properties'],
    ['pyproject.toml', 'toml'],
    ['app.log', 'log'],
    ['app.log.1', 'log'],
    ['notes.txt', 'text'],
    ['build.mk', 'makefile'],
    ['UPPER.PHP', 'php'],
  ] as const)('%s → %s', (path, language) => {
    expect(detectLanguage(path)).toBe(language);
  });

  it('treats a leading dot alone as part of the name, not an extension', () => {
    expect(detectLanguage('.gitignore')).toBe('text');
    expect(detectLanguage('/home/deploy/.ssh/config')).toBe('text');
  });
});

describe('detectLanguage: first line', () => {
  it.each([
    ['#!/bin/sh', 'shell'],
    ['#!/bin/bash', 'shell'],
    ['#!/usr/bin/env bash', 'shell'],
    ['#!/usr/bin/env python3', 'python'],
    ['#!/usr/bin/python3.12', 'python'],
    ['#!/usr/bin/env -S node --no-warnings', 'javascript'],
    ['#!/usr/bin/env php', 'php'],
    ['#! /usr/bin/env zsh', 'shell'],
    ['<?php', 'php'],
    ['<?xml version="1.0"?>', 'xml'],
    ['<!DOCTYPE html>', 'html'],
    ['﻿#!/bin/sh', 'shell'],
  ] as const)('%s → %s', (line, language) => {
    expect(detectLanguage('scripts/migrate', line)).toBe(language);
  });

  it('prefers a known extension over the first line', () => {
    expect(detectLanguage('script.py', '#!/bin/sh')).toBe('python');
  });

  it('lets the first line refine plain text files', () => {
    expect(detectLanguage('notes.txt', '#!/bin/sh')).toBe('shell');
  });

  it('falls back to plain text', () => {
    expect(detectLanguage('LICENSE')).toBe('text');
    expect(detectLanguage('data.bin', 'garbage')).toBe('text');
    expect(detectLanguage('runner', '#!/usr/bin/env ruby')).toBe('text');
  });
});

describe('the language table', () => {
  it('has a label for every language and lists them all in the picker, plain text last', () => {
    for (const id of LANGUAGE_IDS) expect(LANGUAGE_LABEL[id]).toBeTruthy();
    expect(LANGUAGE_OPTIONS.map((option) => option.id).sort()).toEqual([...LANGUAGE_IDS].sort());
    expect(LANGUAGE_OPTIONS.at(-1)?.id).toBe('text');
    const labels = LANGUAGE_OPTIONS.slice(0, -1).map((option) => option.label.toLowerCase());
    expect(labels).toEqual([...labels].sort((a, b) => a.localeCompare(b, 'en')));
  });

  it('covers the v1 languages of plan §2.3', () => {
    const v1 = [
      'php',
      'go',
      'yaml',
      'json',
      'javascript',
      'typescript',
      'html',
      'css',
      'scss',
      'xml',
      'sql',
      'python',
      'rust',
      'shell',
      'markdown',
      'ini',
      'dotenv',
      'properties',
      'toml',
      'nginx',
      'dockerfile',
      'apache',
      'text',
      'log',
    ];
    for (const id of v1) expect(isLanguageId(id)).toBe(true);
    expect(isLanguageId('cobol')).toBe(false);
  });

  it('reads the first line cheaply', () => {
    expect(firstLineOf('#!/bin/sh\necho hi')).toBe('#!/bin/sh');
    expect(firstLineOf('single')).toBe('single');
    expect(firstLineOf('x'.repeat(2000)).length).toBe(500);
  });
});
