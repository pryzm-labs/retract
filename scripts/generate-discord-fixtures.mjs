// Hand-authored synthetic source only. Never reads an external archive or report.
import { createHash } from 'node:crypto';
import { mkdirSync, writeFileSync } from 'node:fs';
import { fileURLToPath } from 'node:url';
import { resolve } from 'node:path';

export const fixtureDirectory = fileURLToPath(new URL('../src-tauri/test-fixtures/discord-import/', import.meta.url));
export const sha256 = bytes => createHash('sha256').update(bytes).digest('hex');
const json = value => JSON.stringify(value, null, 2) + '\n';
const owner = { id: '9007199254741001', username: 'invented_owner', global_name: 'Invented Owner' };
const timestamp = '2030-01-02 03:04:05';
const row = (id, Contents, Attachments = '') => ({ ID: id, Timestamp: timestamp, Contents, Attachments });
const contexts = [
  { header: { id: '9007199254741101', type: 'invented_direct', recipients: [owner.id, '9007199254741002'] },
    rows: [row('9007199254741201', 'Invented hello 🌱\nSecond invented line.')] },
  { header: { id: '9007199254741102', type: 'invented_group', name: 'Invented Group', recipients: [owner.id, '9007199254741002', 'invented_unresolved_recipient'] },
    rows: [row('9007199254741202', 'Invented group text.')] },
  { header: { id: '9007199254741103', type: 'invented_guild', name: 'Invented Channel', guild: { id: '9007199254741301', name: 'Invented Guild' } },
    rows: [row('9007199254741203', 'Invented channel text.', 'https://example.invalid/invented-file.png')] },
  { header: { id: '9007199254741104', type: 'invented_unknown' }, rows: [] },
  { header: { id: '9007199254741105', type: 'invented_direct', name: null, recipients: ['9007199254741002'] },
    rows: [row('9007199254741204', '', 'https://example.invalid/invented-attachment.txt')] },
];

export function crc32(bytes) {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let bit = 0; bit < 8; bit++) crc = (crc >>> 1) ^ ((crc & 1) ? 0xedb88320 : 0);
  }
  return (crc ^ 0xffffffff) >>> 0;
}
export function storedZip(entries) {
  const locals = [], central = [];
  let offset = 0;
  for (const [name, payload] of entries) {
    const filename = Buffer.from(name), body = Buffer.from(payload), crc = crc32(body);
    const local = Buffer.alloc(30), directory = Buffer.alloc(46);
    local.writeUInt32LE(0x04034b50); local.writeUInt16LE(20, 4); local.writeUInt16LE(33, 12);
    local.writeUInt32LE(crc, 14); local.writeUInt32LE(body.length, 18); local.writeUInt32LE(body.length, 22); local.writeUInt16LE(filename.length, 26);
    directory.writeUInt32LE(0x02014b50); directory.writeUInt16LE(0x0314, 4); directory.writeUInt16LE(20, 6); directory.writeUInt16LE(33, 14);
    directory.writeUInt32LE(crc, 16); directory.writeUInt32LE(body.length, 20); directory.writeUInt32LE(body.length, 24); directory.writeUInt16LE(filename.length, 28);
    directory.writeUInt32LE((0o100644 << 16) >>> 0, 38); directory.writeUInt32LE(offset, 42);
    locals.push(local, filename, body); central.push(directory, filename); offset += local.length + filename.length + body.length;
  }
  const directory = Buffer.concat(central), end = Buffer.alloc(22);
  end.writeUInt32LE(0x06054b50); end.writeUInt16LE(entries.length, 8); end.writeUInt16LE(entries.length, 10);
  end.writeUInt32LE(directory.length, 12); end.writeUInt32LE(offset, 16);
  return Buffer.concat([...locals, directory, end]);
}

export function artifacts() {
  const index = Object.fromEntries(contexts.map(({ header }) => [header.id, header.name ?? 'Invented unnamed context']));
  const entries = [['Account/user.json', json(owner)], ['Messages/index.json', json(index)]];
  const expectedRows = [];
  for (const { header, rows } of contexts) {
    entries.push([`Messages/c${header.id}/channel.json`, json(header)]);
    // IDs never pass through JavaScript Number. Only this validated decimal is
    // emitted as an original JSON integer token; every other field uses JSON.
    const payload = '[\n' + rows.map(({ ID, ...rest }) => {
      if (!/^[1-9][0-9]*$/.test(ID) || BigInt(ID) <= 9007199254740991n || BigInt(ID) > 18446744073709551615n) throw new Error('invalid synthetic ID');
      return `  {"ID":${ID},${JSON.stringify(rest).slice(1)}`;
    }).join(',\n') + '\n]\n';
    entries.push([`Messages/c${header.id}/messages.json`, payload]);
    for (const record of rows) expectedRows.push({ accountId: owner.id, channelId: header.id, messageId: record.ID,
      timestamp: { lexeme: record.Timestamp, calendar: { year: 2030, month: 1, day: 2, hour: 3, minute: 4, second: 5 }, zone: 'unzoned', normalization: 'unresolved' },
      text: record.Contents, attachmentEncoding: record.Attachments });
  }
  const expected = json({ schemaKey: 'discord.data_package.messages_json', schemaVersion: 1, policyKey: 'discord.import_policy.v1',
    contextOrder: contexts.map(({ header }) => header.id), records: expectedRows });
  const strings = new Set(), numbers = new Set(), keys = new Set();
  function collect(value) {
    if (typeof value === 'string') strings.add(value);
    else if (typeof value === 'number') numbers.add(String(value));
    else if (Array.isArray(value)) value.forEach(collect);
    else if (value && typeof value === 'object') for (const [key, child] of Object.entries(value)) { keys.add(key); collect(child); }
  }
  collect(owner); collect(index); contexts.forEach(({ header, rows }) => { collect(header); collect(rows); rows.forEach(row => numbers.add(row.ID)); });
  collect(JSON.parse(expected));
  const zip = storedZip(entries);
  const manifest = json({ observationDate: '2026-09-10',
    documentationUrl: 'https://support.discord.com/hc/en-us/articles/360004957991-Your-Discord-Data-Package',
    provenance: 'Hand-authored synthetic data. No source values were copied. Hashes cover only synthetic artifacts.',
    generatorCommand: 'node scripts/generate-discord-fixtures.mjs (Docker ARM64, UID 10001, offline)',
    schemaKey: 'discord.data_package.messages_json', schemaVersion: 1, policyKey: 'discord.import_policy.v1',
    profile: { roots: ['Account/user.json', 'Messages/index.json', 'Messages/c<canonical-positive-u64>/channel.json', 'Messages/c<canonical-positive-u64>/messages.json'],
      account: { root: 'object', required: { id: 'canonical positive u64 decimal string', username: 'string' } },
      index: { root: 'object', interpretation: 'bounded ignored mapping; no identity authority' },
      channel: { root: 'object', required: { id: 'canonical positive u64 decimal string matching path', type: 'opaque string' },
        optional: { name: 'absent|string|null', recipients: 'absent|array of opaque strings', guild: 'absent|object with id decimal string and name string' },
        constraints: 'guild requires a string channel name and excludes recipients; no friendly semantic discriminator mapping' },
      messages: { root: 'array of objects, including empty', required: { ID: 'original canonical positive u64 JSON integer token', Timestamp: 'calendar-valid YYYY-MM-DD HH:MM:SS, unzoned', Contents: 'string', Attachments: 'opaque string' } },
      unknownFields: 'bounded and ignored', timestampPolicy: 'Fixed synthetic calendar components are unzoned. No UTC instant, host-local timezone or normalization is claimed. Task 3 must establish authoritative UTC interpretation or reject this profile.' },
    approvedUsernames: [owner.username], approvedStrings: [...strings].sort(), approvedNumbers: [...numbers].sort(), approvedKeys: [...keys].sort(),
    files: Object.fromEntries(entries.map(([name, body]) => [name, sha256(body)])), zipSha256: sha256(zip), expectedRecordsSha256: sha256(expected) });
  return { entries, zip, expected, manifest };
}

if (process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const generated = artifacts();
  mkdirSync(fixtureDirectory, { recursive: true });
  for (const [filename, data] of [['current-json.zip', generated.zip], ['expected-records.json', generated.expected], ['manifest.json', generated.manifest]]) writeFileSync(resolve(fixtureDirectory, filename), data);
  console.log('Synthetic Discord fixtures generated deterministically.');
}
