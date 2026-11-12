import { appendFileSync, copyFileSync, mkdirSync, readFileSync, readdirSync, writeFileSync } from 'node:fs';
import { basename, join, resolve } from 'node:path';
import { execFileSync } from 'node:child_process';
import { createHash } from 'node:crypto';
import { pathToFileURL } from 'node:url';

function git(root, ...args) {
  return execFileSync('git', ['-C', root, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim();
}

export function resolveTag(root, tag) {
  if (!/^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(tag ?? '')) {
    throw new Error('Release tag must be an existing vMAJOR.MINOR.PATCH tag');
  }
  let sha;
  try { sha = git(root, 'rev-parse', '--verify', `refs/tags/${tag}^{commit}`); }
  catch { throw new Error(`Release tag does not exist: ${tag}`); }
  return { tag, version: tag.slice(1), sha };
}

export function validateSource(root, tag, expectedSha) {
  const release = resolveTag(root, tag);
  if (release.sha !== expectedSha || git(root, 'rev-parse', 'HEAD') !== expectedSha) {
    throw new Error('Source commit does not match the resolved release tag commit');
  }
  const json = (file) => JSON.parse(readFileSync(join(root, file), 'utf8'));
  const lock = json('package-lock.json');
  const cargo = readFileSync(join(root, 'src-tauri/Cargo.toml'), 'utf8');
  const packageSection = cargo.match(/^\[package\]\s*$([\s\S]*?)(?=^\[|$(?![\s\S]))/m)?.[1];
  const cargoVersion = packageSection?.match(/^version\s*=\s*"([^"]+)"\s*$/m)?.[1];
  const versions = {
    'package.json': json('package.json').version,
    'package-lock.json': lock.version,
    'package-lock.json packages[""]': lock.packages?.['']?.version,
    'Cargo.toml': cargoVersion,
    'tauri.conf.json': json('src-tauri/tauri.conf.json').version,
  };
  for (const [file, version] of Object.entries(versions)) {
    if (version !== release.version) throw new Error(`${file} version must equal ${release.version}`);
  }
  // npm ci and the CLI must not rewrite tracked source or either lockfile.
  try { git(root, 'diff', '--exit-code', 'HEAD', '--', '.'); }
  catch { throw new Error('Tracked source differs from the release tag commit'); }
  return release;
}

function files(root, extension) {
  return readdirSync(root, { withFileTypes: true }).flatMap((entry) => {
    const path = join(root, entry.name);
    return entry.isDirectory() ? files(path, extension) : entry.isFile() && entry.name.endsWith(extension) ? [path] : [];
  });
}

export function collectAssets(bundleDir, outputDir, version) {
  if (!/^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.test(version ?? '')) throw new Error('Invalid asset version');
  const appimages = files(bundleDir, '.AppImage');
  const debs = files(bundleDir, '.deb');
  if (appimages.length !== 1 || debs.length !== 1) throw new Error('Expected exactly one AppImage and one deb package');
  const [appimage] = appimages;
  const [deb] = debs;
  for (const path of [appimage, deb]) {
    if (!basename(path).includes(`_${version}_`)) throw new Error('Bundle filename version does not match tag version');
  }
  const header = readFileSync(appimage).subarray(0, 20);
  if (header.length < 20 || header.subarray(0, 6).toString('hex') !== '7f454c460201'
      || header.subarray(8, 11).toString('hex') !== '414902' || header.readUInt16LE(18) !== 62) {
    throw new Error('AppImage must be a type-2 Linux x86_64 ELF executable');
  }
  const debField = (name) => execFileSync('dpkg-deb', ['--field', deb, name], { encoding: 'utf8' }).trim();
  if (debField('Version') !== version) throw new Error('Debian package version does not match tag version');
  if (debField('Architecture') !== 'amd64') throw new Error('Debian package architecture must be amd64');
  // Refuse a corrupt data archive even if its control metadata looks valid.
  execFileSync('dpkg-deb', ['--contents', deb], { stdio: 'pipe' });
  mkdirSync(outputDir, { recursive: true });
  const assets = [];
  const checksums = [];
  for (const [source, extension] of [[appimage, 'AppImage'], [deb, 'deb']]) {
    const name = `OpsPier_${version}_linux_x86_64.${extension}`;
    copyFileSync(source, join(outputDir, name));
    checksums.push(`${createHash('sha256').update(readFileSync(source)).digest('hex')}  ${name}`);
    assets.push(name);
  }
  const checksumName = 'SHA256SUMS-linux-x86_64.txt';
  writeFileSync(join(outputDir, checksumName), `${checksums.join('\n')}\n`);
  return [...assets, checksumName];
}

if (process.argv[1] && import.meta.url === pathToFileURL(resolve(process.argv[1])).href) {
  try {
    const [command, root, tagOrOutput, shaOrVersion] = process.argv.slice(2);
    const result = command === 'resolve-tag' ? resolveTag(root, tagOrOutput)
      : command === 'validate-source' ? validateSource(root, tagOrOutput, shaOrVersion)
      : command === 'collect-assets' ? collectAssets(root, tagOrOutput, shaOrVersion)
      : (() => { throw new Error('Expected resolve-tag, validate-source or collect-assets'); })();
    if (command === 'resolve-tag' && process.env.GITHUB_OUTPUT) {
      appendFileSync(process.env.GITHUB_OUTPUT, Object.entries(result).map(([key, value]) => `${key}=${value}\n`).join(''));
    }
    console.log(JSON.stringify(result));
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}
