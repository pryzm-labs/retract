declare const brand: unique symbol;
export type Branded<Name extends string> = string & { readonly [brand]: Name };
export type ProviderKey = Branded<"ProviderKey">;
export type AccountId = Branded<"AccountId">;
export type SourceId = Branded<"SourceId">;
export type ConversationId = Branded<"ConversationId">;
export type ContentId = Branded<"ContentId">;
export type ActorId = Branded<"ActorId">;
export type ResourceId = Branded<"ResourceId">;
export type Uuid = Branded<"Uuid">;
export interface Scope { provider: ProviderKey; accountId: AccountId; sourceId: SourceId }
export interface ActiveContext { scope: Scope; sessionGeneration: Uuid }
export type ResourceKind = "conversation" | "content" | "actor" | "grouping";
export interface ProviderResourceRef {
  provider: ProviderKey; accountId: AccountId; resourceKind: ResourceKind;
  locatorSchema: string; locatorVersion: number; canonicalKey: string; locatorPayload: unknown;
}
export interface ScopedResourceRef { scope: Scope; id: ResourceId; resource: ProviderResourceRef }
export const scopeKey = (scope: Scope) => JSON.stringify([scope.provider, scope.accountId, scope.sourceId]);
export const resourceKey = (scope: Scope, kind: ResourceKind, id: string) => JSON.stringify([scope.provider, scope.accountId, scope.sourceId, kind, id]);
export const refKey = (ref: ScopedResourceRef) => resourceKey(ref.scope, ref.resource.resourceKind, ref.id);
export const sameScope = (a: Scope, b: Scope) => scopeKey(a) === scopeKey(b);
export const sameContext = (a: ActiveContext | null, b: ActiveContext | null) => a === null || b === null
  ? a === b : sameScope(a.scope, b.scope) && a.sessionGeneration === b.sessionGeneration;
export function uuid<Name extends string = "Uuid">(value: unknown): Branded<Name> {
  if (typeof value !== "string" || !/^[0-9a-f]{8}-[0-9a-f]{4}-[1-8][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value)) throw new Error("Invalid application identity.");
  return value as Branded<Name>;
}
export function providerKey(value: unknown): ProviderKey {
  if (typeof value !== "string" || !/^[a-z][a-z0-9_-]{0,63}$/.test(value)) throw new Error("Invalid provider identity.");
  return value as ProviderKey;
}
// SHA-1 is used only for the UUIDv5 identity standard, never authorization.
// Operates on UTF-8 canonical envelope bytes, never native locator fields.
export function resourceId(resource: ProviderResourceRef): ResourceId {
  const hex = "0123456789abcdef";
  const namespace = resource.accountId.replaceAll("-", "");
  const bytes = Array.from({ length: 16 }, (_, i) => hex.indexOf(namespace[i * 2]) * 16 + hex.indexOf(namespace[i * 2 + 1]));
  bytes.push(...new TextEncoder().encode(JSON.stringify(["retract-resource-v1", resource.resourceKind, resource.locatorSchema, resource.locatorVersion, resource.canonicalKey])));
  const bitLength = bytes.length * 8;
  bytes.push(0x80);
  while (bytes.length % 64 !== 56) bytes.push(0);
  bytes.push(0, 0, 0, 0, (bitLength >>> 24) & 255, (bitLength >>> 16) & 255, (bitLength >>> 8) & 255, bitLength & 255);
  const h = [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476, 0xc3d2e1f0];
  const rol = (n: number, bits: number) => (n << bits) | (n >>> (32 - bits));
  for (let offset = 0; offset < bytes.length; offset += 64) {
    const w = Array<number>(80);
    for (let i = 0; i < 16; i++) { const p = offset + i * 4; w[i] = (bytes[p] << 24) | (bytes[p + 1] << 16) | (bytes[p + 2] << 8) | bytes[p + 3]; }
    for (let i = 16; i < 80; i++) w[i] = rol(w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16], 1);
    let [a, b, c, d, e] = h;
    for (let i = 0; i < 80; i++) {
      const f = i < 20 ? (b & c) | (~b & d) : i < 40 ? b ^ c ^ d : i < 60 ? (b & c) | (b & d) | (c & d) : b ^ c ^ d;
      const k = i < 20 ? 0x5a827999 : i < 40 ? 0x6ed9eba1 : i < 60 ? 0x8f1bbcdc : 0xca62c1d6;
      const temp = (rol(a, 5) + f + e + k + w[i]) | 0;
      e = d; d = c; c = rol(b, 30); b = a; a = temp;
    }
    [a, b, c, d, e].forEach((n, i) => { h[i] = (h[i] + n) | 0; });
  }
  const digest = h.flatMap(n => [(n >>> 24) & 255, (n >>> 16) & 255, (n >>> 8) & 255, n & 255]).slice(0, 16);
  digest[6] = (digest[6] & 15) | 80; digest[8] = (digest[8] & 63) | 128;
  const encoded = digest.map(n => hex[n >>> 4] + hex[n & 15]).join("");
  return uuid<"ResourceId">(`${encoded.slice(0, 8)}-${encoded.slice(8, 12)}-${encoded.slice(12, 16)}-${encoded.slice(16, 20)}-${encoded.slice(20)}`);
}
export const avatarSeed = (id: string) => Array.from(id).reduce((hash, char) => ((hash * 31) + char.charCodeAt(0)) >>> 0, 0) % 19;
