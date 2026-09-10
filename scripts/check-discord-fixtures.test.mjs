import test from 'node:test';
import assert from 'node:assert/strict';
import { auditValue, auditJson, auditFixtureArtifacts } from './check-discord-fixtures.mjs';
import { artifacts, sha256 } from './generate-discord-fixtures.mjs';

const manifest = { approvedStrings: ['invented_owner', 'invented text', 'https://example.invalid/file'], approvedUsernames: ['invented_owner'], approvedNumbers: ['9007199254741201'] };
test('privacy audit accepts only explicitly approved synthetic values', () => {
  for (const value of manifest.approvedStrings) assert.doesNotThrow(() => auditValue(value, '', manifest));
  assert.doesNotThrow(() => auditValue('invented_owner', 'username', manifest));
  assert.throws(() => auditValue('invented text', 'username', manifest));
  assert.throws(() => auditValue('unapproved_sentinel', '', manifest));
});
test('deterministic artifacts, provenance and all synthetic scalar values pass', () => {
  const first = artifacts(), second = artifacts();
  assert.deepEqual(first, second);
  assert.doesNotThrow(() => auditFixtureArtifacts(first));
});
test('artifact mutations cannot be approved by merely updating hashes', () => {
  const original = artifacts();
  for (const changes of [
    { zip: Buffer.concat([original.zip, Buffer.from('unapproved_sentinel')]) },
    { expected: original.expected.replace('Invented group text.', 'unapproved_sentinel') },
    { manifest: original.manifest.replace('invented_owner', 'unexpected_username') },
  ]) {
    const changed = { ...original, ...changes }, manifest = JSON.parse(changed.manifest);
    manifest.zipSha256 = sha256(changed.zip); manifest.expectedRecordsSha256 = sha256(changed.expected);
    changed.manifest = JSON.stringify(manifest, null, 2) + '\n';
    assert.throws(() => auditFixtureArtifacts(changed));
  }
});
test('numeric audit is lossless and unknown keys are forbidden', () => {
  const policy = { ...manifest, approvedKeys: ['ID'] };
  assert.doesNotThrow(() => auditJson('{"ID":9007199254741201}', policy));
  for (const body of ['{"ID":9007199254741200}', '{"ID":9.007199254741201e15}', '{"unknown":9007199254741201}']) assert.throws(() => auditJson(body, policy));
});
test('all fixture identity strings exceed the safe-integer boundary', () => {
  for (const value of ['1', '9007199254740991', '09007199254741001', '18446744073709551616']) {
    assert.throws(() => auditValue(value, 'id', { ...manifest, approvedStrings: [...manifest.approvedStrings, value] }));
  }
});
test('sensitive patterns stay forbidden even when added to the approved manifest', () => {
  for (const value of ['invented@example.invalid', '+1 202 555 0100', '202-555-0100', '192.0.2.1', '2001:db8::1', 'Invented 192.0.2.1.', 'Invented 2001:db8::1!', 'https://other.example.invalid/file', 'https://example.invalid@other.example.invalid/file', 'javascript:invented', 'file:///invented']) {
    assert.throws(() => auditValue(value, '', { ...manifest, approvedStrings: [...manifest.approvedStrings, value] }));
  }
});
