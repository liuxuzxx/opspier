import test from 'node:test';
import assert from 'node:assert/strict';
import { boundedPage, csvCell, metadataOnly, QueryGeneration } from '../src/db-model.ts';

test('page rejects invalid input and clamps a requested page to cached rows', () => {
  assert.deepEqual(boundedPage(999, 1000, 23), { offset: 0, limit: 200 });
  assert.deepEqual(boundedPage(21, 10, 23), { offset: 20, limit: 10 });
  assert.throws(() => boundedPage(-1, 10, 3));
  assert.throws(() => boundedPage(0, 0, 3));
});
test('CSV quotes values and neutralizes spreadsheet formulas', () => {
  assert.equal(csvCell('=HYPERLINK("bad")'), '"\'=HYPERLINK(""bad"")"');
  assert.equal(csvCell(' a,b\n'), '" a,b\n"');
  assert.equal(csvCell(null), '""');
});
test('connection persistence strips credentials and keeps TLS metadata', () => {
  const result = metadataOnly({ host: 'db.test', port: 3306, username: 'reader', password: 'secret', database: 'demo', kind: 'doris', tls: 'verifyIdentity', name: 'demo' });
  assert.equal('password' in result, false);
  assert.equal(result.tls, 'verifyIdentity');
  assert.equal(result.kind, 'doris');
});
test('older query completion cannot replace a newer result', () => {
  const generation = new QueryGeneration();
  const old = generation.next(); const current = generation.next();
  assert.equal(generation.isCurrent(old), false);
  assert.equal(generation.isCurrent(current), true);
  generation.invalidate();
  assert.equal(generation.isCurrent(current), false);
});
