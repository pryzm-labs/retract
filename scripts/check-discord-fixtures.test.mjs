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

function approved(value) { return { ...manifest, approvedStrings: [...manifest.approvedStrings, value] }; }
test('parenthesized and embedded phone formats cannot be auto-approved', () => {
  for (const value of ['(202) 555-0100', 'Call (202)555-0100 now', 'Call 2025550100 now', 'x2025550100y', '+1(202)5550100']) {
    assert.throws(() => auditValue(value, '', approved(value)), /^Error: synthetic fixture privacy audit failed$/);
  }
});
test('numeric IPv6 is detected through quoting punctuation and CIDR suffixes', () => {
  for (const value of ['"2001:4860:4860::8888"', 'Address 2001:4860:4860::8888/64', 'Address ::1:', 'Address 2001:4860:4860::8888:']) {
    assert.throws(() => auditValue(value, '', approved(value)), /^Error: synthetic fixture privacy audit failed$/);
  }
});
test('IPv4 addresses with ports and embedded text cannot be auto-approved', () => {
  for (const value of ['192.0.2.1:80', 'Address 192.0.2.1:80 now', '"198.51.100.2:443"',
    '[203.0.113.3]:8080', '192.0.2.1:80/24', 'a192.0.2.1z']) {
    assert.throws(() => auditValue(value, '', approved(value)), /^Error: synthetic fixture privacy audit failed$/);
  }
});
test('IPv4 addresses with ports cannot be approved as JSON keys', () => {
  for (const key of ['192.0.2.1:80', 'Address 198.51.100.2:443 now', 'a203.0.113.3z']) {
    assert.throws(() => auditJson(JSON.stringify({ [key]: null }), { ...approved(key), approvedKeys: [key] }),
      /^Error: synthetic fixture privacy audit failed$/);
  }
});
test('IPv4 candidate validation does not mistake invalid octets or longer numbers for addresses', () => {
  for (const value of ['999.0.2.1:80', '192.0.2.999:80', '1192.0.2.1:80', '192.0.2.1111:80', 'Invented 192.0.2 item']) {
    assert.doesNotThrow(() => auditValue(value, '', approved(value)));
    assert.doesNotThrow(() => auditJson(JSON.stringify({ [value]: null }), { ...approved(value), approvedKeys: [value] }));
  }
});
test('URL authorities reject relative externals credentials and even default ports', () => {
  for (const value of ['//outside.example.invalid/file', '//name@example.invalid/file', '//example.invalid:443/file',
    'https://example.invalid:443/file', 'http://example.invalid:80/file', 'ftp://example.invalid/file', '//example.invalid\\outside', 'http:example.invalid/file']) {
    assert.throws(() => auditValue(value, '', approved(value)), /^Error: synthetic fixture privacy audit failed$/);
  }
  for (const value of ['//example.invalid/file', 'https://example.invalid/file', 'http://example.invalid/file']) assert.doesNotThrow(() => auditValue(value, '', approved(value)));
});
test('sensitive JSON keys are checked even when included in approvedKeys', () => {
  for (const key of ['invented@example.invalid', '(202) 555-0100', 'Address 2001:4860:4860::8888/64', '//outside.example.invalid/file']) {
    const policy = { ...approved(key), approvedKeys: [key] };
    assert.throws(() => auditJson(JSON.stringify({ [key]: null }), policy), /^Error: synthetic fixture privacy audit failed$/);
  }
});
test('near-miss synthetic text keys and fixed calendar components remain safe', () => {
  for (const value of ['Invented item 42', '2030-01-02 03:04:05', '9007199254741001', 'Invented (202) note', 'Invented 202-555 item', 'Invented 03:04:05 clock']) {
    assert.doesNotThrow(() => auditValue(value, '', approved(value)));
    assert.doesNotThrow(() => auditJson(JSON.stringify({ [value]: null }), { ...approved(value), approvedKeys: [value] }));
  }
});
