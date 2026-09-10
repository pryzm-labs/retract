import { readFileSync, readdirSync } from 'node:fs';
import { isIP } from 'node:net';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { artifacts, fixtureDirectory, sha256 } from './generate-discord-fixtures.mjs';

function reject() { throw new Error('synthetic fixture privacy audit failed'); }
export function auditValue(value, key, manifest) {
  if (typeof value !== 'string') return;
  if (!manifest.approvedStrings.includes(value)) reject();
  if (['id', 'ID', 'accountId', 'channelId', 'messageId'].includes(key) || /^\d+$/.test(value)) {
    if (!/^[1-9][0-9]*$/.test(value) || BigInt(value) <= 9007199254740991n || BigInt(value) > 18446744073709551615n) reject();
  }
  if (key === 'username' && !manifest.approvedUsernames.includes(value)) reject();
  if (/[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}/u.test(value) || /\+\d[\d ()-]{7,}\d/u.test(value) || /\b\d{3}[-. ]\d{3}[-. ]\d{4}\b/u.test(value) || /^\d{10,15}$/.test(value)) reject();
  if (value.split(/[\s,;()[\]]+/u).some(token => isIP(token.replace(/[.!?]+$/u, '')) !== 0)) reject();
  if ([...value.matchAll(/\b(?:\d{1,3}\.){3}\d{1,3}\b/gu)].some(match => isIP(match[0]) === 4)) reject();
  for (const match of value.matchAll(/[A-Za-z][A-Za-z0-9+.-]*:[^\s]*/gu)) {
    let url; try { url = new URL(match[0]); } catch { reject(); }
    if (!['https:', 'http:'].includes(url.protocol) || url.hostname !== 'example.invalid' || url.username || url.password || url.port) reject();
  }
}
// Lex each numeric token BEFORE JSON.parse so large IDs never round. The
// subsequent parse sees only placeholders and is used solely for key/string audit.
export function auditJson(body, manifest) {
  const tokens = body.match(/"(?:[^"\\]|\\.)*"|-?\d+(?:\.\d+)?(?:[eE][+-]?\d+)?|[^"\d-]+/gu);
  if (!tokens || tokens.join('') !== body) reject();
  const safe = tokens.map(token => {
    if (!/^-?\d/.test(token)) return token;
    if (!manifest.approvedNumbers.includes(token)) reject();
    return 'null';
  }).join('');
  let value; try { value = JSON.parse(safe); } catch { reject(); }
  function walk(value, key = '') {
    if (typeof value === 'string') auditValue(value, key, manifest);
    else if (Array.isArray(value)) value.forEach(child => walk(child));
    else if (value && typeof value === 'object') for (const [key, child] of Object.entries(value)) {
      if (!manifest.approvedKeys.includes(key)) reject(); walk(child, key);
    }
  }
  walk(value);
}
export function auditFixtureArtifacts(actual) {
  const generated = artifacts();
  // Byte equality with the tiny hand-authored generator audits the complete ZIP,
  // including filenames, wrappers, scalar values, metadata and trailing bytes.
  // A changed artifact cannot be authorized just by updating its hash.
  if (!actual.zip.equals(generated.zip) || actual.expected !== generated.expected || actual.manifest !== generated.manifest) reject();
  const manifest = JSON.parse(actual.manifest);
  if (manifest.zipSha256 !== sha256(actual.zip) || manifest.expectedRecordsSha256 !== sha256(actual.expected)) reject();
  for (const [name, body] of generated.entries) {
    if (manifest.files[name] !== sha256(body)) reject(); auditJson(body, manifest);
  }
  auditJson(actual.expected, manifest);
}
export function checkDiscordFixtures() {
  const files = readdirSync(fixtureDirectory).sort();
  if (files.join('|') !== ['README.md', 'current-json.zip', 'expected-records.json', 'manifest.json'].join('|')) reject();
  auditFixtureArtifacts({ zip: readFileSync(resolve(fixtureDirectory, 'current-json.zip')),
    expected: readFileSync(resolve(fixtureDirectory, 'expected-records.json'), 'utf8'), manifest: readFileSync(resolve(fixtureDirectory, 'manifest.json'), 'utf8') });
}
if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  try { checkDiscordFixtures(); console.log('Synthetic Discord fixture provenance and privacy audit passed.'); }
  catch { console.error('Synthetic Discord fixture audit failed.'); process.exitCode = 1; }
}
