import { readFileSync, readdirSync } from 'node:fs';
import { isIP } from 'node:net';
import { resolve } from 'node:path';
import { fileURLToPath } from 'node:url';
import { artifacts, fixtureDirectory, sha256 } from './generate-discord-fixtures.mjs';

function reject() { throw new Error('synthetic fixture privacy audit failed'); }
function auditSensitiveContent(value) {
  if (/[\w.+-]+@[\w.-]+\.[A-Za-z]{2,}/u.test(value)
    || /\+\d[\d ().-]{7,}\d/u.test(value)
    || /(?<!\d)\(\d{3}\)[\s.-]*\d{3}[\s.-]*\d{4}(?!\d)/u.test(value)
    || /(?<!\d)\d{3}[-. ]+\d{3}[-. ]*\d{4}(?!\d)/u.test(value)
    || /(?<!\d)\d{10,15}(?!\d)/u.test(value)) reject();
  // Scan IPv4 independently: a port or adjacent hex letter would otherwise
  // become part of the broader IPv6 candidate and conceal a valid address.
  for (const match of value.matchAll(/(?<!\d)(?:\d{1,3}\.){3}\d{1,3}(?!\d)/gu)) {
    if (isIP(match[0]) === 4) reject();
  }
  // Extract numeric-address candidates independently of word/punctuation tokens.
  // A final colon can be prose punctuation; try removing just that delimiter,
  // without collapsing internal IPv6 colons or treating calendar times as IPs.
  for (const match of value.matchAll(/[0-9A-Fa-f:.]+/gu)) {
    const candidate = match[0].replace(/\.+$/u, '');
    if ([candidate, candidate.replace(/^:/u, ''), candidate.replace(/:$/u, '')].some(part => isIP(part) !== 0)) reject();
  }
  for (const match of value.matchAll(/(?:[A-Za-z][A-Za-z0-9+.-]*:)?\/\/[^\s<>"']*|[A-Za-z][A-Za-z0-9+.-]*:[^\s<>"']*/gu)) {
    const candidate = match[0];
    const authority = candidate.match(/^(?:https?:)?\/\/([^/?#]*)/iu)?.[1];
    // Inspect the original authority too: URL normalizes explicit default ports
    // away, which must not let a port or credential spelling bypass this policy.
    if (authority?.toLowerCase() !== 'example.invalid' || candidate.includes('\\')) reject();
    let url; try { url = new URL(candidate.startsWith('//') ? `https:${candidate}` : candidate); } catch { reject(); }
    if (!['https:', 'http:'].includes(url.protocol) || url.hostname !== 'example.invalid' || url.username || url.password || url.port) reject();
  }
}
export function auditValue(value, key, manifest) {
  if (typeof value !== 'string') return;
  auditSensitiveContent(value);
  if (!manifest.approvedStrings.includes(value)) reject();
  if (['id', 'ID', 'accountId', 'channelId', 'messageId'].includes(key) || /^\d+$/.test(value)) {
    if (!/^[1-9][0-9]*$/.test(value) || BigInt(value) <= 9007199254740991n || BigInt(value) > 18446744073709551615n) reject();
  }
  if (key === 'username' && !manifest.approvedUsernames.includes(value)) reject();
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
      auditSensitiveContent(key);
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
