import test from 'node:test';
import assert from 'node:assert/strict';
import { mkdtempSync, mkdirSync, writeFileSync, readFileSync, rmSync, copyFileSync, chmodSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { execFileSync } from 'node:child_process';
import { resolveTag, validateSource, collectAssets } from '../scripts/release/linux-release.mjs';

function fixture(t) {
  const root = mkdtempSync(join(tmpdir(), 'opspier-release-test-'));
  t.after(() => rmSync(root, { recursive: true, force: true }));
  return root;
}
function git(root, ...args) { return execFileSync('git', ['-C', root, ...args], { encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] }).trim(); }
function source(t) {
  const root = fixture(t);
  mkdirSync(join(root, 'src-tauri'));
  writeFileSync(join(root, 'package.json'), JSON.stringify({ version: '0.1.8' }));
  writeFileSync(join(root, 'package-lock.json'), JSON.stringify({ version: '0.1.8', packages: { '': { version: '0.1.8' } } }));
  writeFileSync(join(root, 'src-tauri/tauri.conf.json'), JSON.stringify({ version: '0.1.8' }));
  writeFileSync(join(root, 'src-tauri/Cargo.toml'), '[package]\nname = "opspier"\nversion = "0.1.8"\n');
  git(root, 'init', '-q');
  git(root, 'add', '.');
  git(root, '-c', 'user.name=Release Test', '-c', 'user.email=release-test@example.invalid', 'commit', '-qm', 'fixture');
  git(root, '-c', 'user.name=Release Test', '-c', 'user.email=release-test@example.invalid', 'tag', '-a', 'v0.1.8', '-m', 'fixture');
  return root;
}
function bundle(t, { version = '0.1.8', arch = 'amd64' } = {}) {
  const root = fixture(t);
  const input = join(root, 'bundle');
  const out = join(root, 'assets');
  mkdirSync(join(input, 'appimage'), { recursive: true });
  mkdirSync(join(input, 'deb'), { recursive: true });
  const image = Buffer.alloc(128);
  image.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
  image.set([0x41, 0x49, 2], 8);
  image.writeUInt16LE(62, 18);
  const appimage = join(input, 'appimage', `OpsPier_${version}_amd64.AppImage`);
  writeFileSync(appimage, image);
  const pkg = join(root, 'pkg');
  mkdirSync(join(pkg, 'DEBIAN'), { recursive: true });
  chmodSync(join(pkg, 'DEBIAN'), 0o755);
  writeFileSync(join(pkg, 'DEBIAN/control'), `Package: opspier\nVersion: ${version}\nArchitecture: ${arch}\nMaintainer: Release Test <release-test@example.invalid>\nDescription: test fixture\n`);
  const deb = join(input, 'deb', `OpsPier_${version}_amd64.deb`);
  execFileSync('dpkg-deb', ['--build', '--root-owner-group', pkg, deb], { stdio: 'pipe' });
  return { input, out, appimage, deb };
}

test('only existing semantic version tags resolve, including annotated tags', (t) => {
  const root = source(t);
  const sha = git(root, 'rev-parse', 'HEAD');
  assert.deepEqual(resolveTag(root, 'v0.1.8'), { tag: 'v0.1.8', version: '0.1.8', sha });
  git(root, 'branch', 'v9.9.9');
  for (const ref of ['main', 'HEAD', sha, 'refs/tags/v0.1.8', 'v9.9.9', 'v0.1.9', 'v01.1.8', 'v0.1.8\nattack']) {
    assert.throws(() => resolveTag(root, ref), /tag/i, ref);
  }
});

test('source validation binds HEAD to tag and rejects mismatched manifest versions', (t) => {
  const root = source(t);
  const { sha } = resolveTag(root, 'v0.1.8');
  assert.equal(validateSource(root, 'v0.1.8', sha).version, '0.1.8');
  assert.throws(() => validateSource(root, 'v0.1.8', '0'.repeat(40)), /commit/i);
  writeFileSync(join(root, 'package.json'), '{"version":"0.1.9"}');
  assert.throws(() => validateSource(root, 'v0.1.8', sha), /version/i);
});

test('source validation rejects different lock, Cargo or Tauri versions', (t) => {
  for (const [file, text] of [
    ['package-lock.json', '{"version":"0.1.8","packages":{"":{"version":"0.1.7"}}}'],
    ['src-tauri/tauri.conf.json', '{"version":"0.1.7"}'],
    ['src-tauri/Cargo.toml', '[package]\nversion = "0.1.7"\n'],
  ]) {
    const root = source(t);
    const { sha } = resolveTag(root, 'v0.1.8');
    writeFileSync(join(root, file), text);
    assert.throws(() => validateSource(root, 'v0.1.8', sha), /version/i, file);
  }
});

test('asset collection emits both packages with independent verifiable checksums', (t) => {
  const { input, out } = bundle(t);
  assert.deepEqual(collectAssets(input, out, '0.1.8'), [
    'OpsPier_0.1.8_linux_x86_64.AppImage', 'OpsPier_0.1.8_linux_x86_64.deb', 'SHA256SUMS-linux-x86_64.txt',
  ]);
  assert.match(execFileSync('sha256sum', ['-c', 'SHA256SUMS-linux-x86_64.txt'], { cwd: out, encoding: 'utf8' }), /\.deb: OK/);
  writeFileSync(join(out, 'OpsPier_0.1.8_linux_x86_64.deb'), 'corrupt');
  assert.throws(() => execFileSync('sha256sum', ['-c', 'SHA256SUMS-linux-x86_64.txt'], { cwd: out, stdio: 'pipe' }));
});

test('missing, empty and duplicate packages fail closed', (t) => {
  const a = bundle(t);
  rmSync(a.deb);
  assert.throws(() => collectAssets(a.input, a.out, '0.1.8'), /exactly one/i);
  const b = bundle(t);
  writeFileSync(b.appimage, '');
  assert.throws(() => collectAssets(b.input, b.out, '0.1.8'), /AppImage/i);
  const c = bundle(t);
  copyFileSync(c.deb, join(c.input, 'deb', 'duplicate.deb'));
  assert.throws(() => collectAssets(c.input, c.out, '0.1.8'), /exactly one/i);
});

test('asset validation rejects wrong version, architecture and invalid AppImage', (t) => {
  const wrongVersion = bundle(t, { version: '0.1.7' });
  assert.throws(() => collectAssets(wrongVersion.input, wrongVersion.out, '0.1.8'), /version/i);
  const wrongArch = bundle(t, { arch: 'arm64' });
  assert.throws(() => collectAssets(wrongArch.input, wrongArch.out, '0.1.8'), /architecture/i);
  const invalidImage = bundle(t);
  writeFileSync(invalidImage.appimage, 'not an executable');
  assert.throws(() => collectAssets(invalidImage.input, invalidImage.out, '0.1.8'), /AppImage/i);
  const armImage = bundle(t);
  const image = readFileSync(armImage.appimage);
  image.writeUInt16LE(183, 18);
  writeFileSync(armImage.appimage, image);
  assert.throws(() => collectAssets(armImage.input, armImage.out, '0.1.8'), /AppImage/i);
});
