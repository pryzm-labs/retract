import { randomFillSync } from 'node:crypto'

export const urlAlphabet = 'useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict'
const normalizeSize = (size) => Math.max(0, Number(size) | 0)

export function nanoid(size = 21) {
  const length = normalizeSize(size)
  if (length === 0) return ''
  const bytes = new Uint8Array(length)
  randomFillSync(bytes)
  let id = ''
  for (let index = 0; index < length; index += 1) id += urlAlphabet[bytes[index] & 63]
  return id
}

export function customAlphabet(alphabet, defaultSize = 21) {
  if (!alphabet || alphabet.length > 256) throw new TypeError('alphabet must contain 1 to 256 symbols')
  return (size = defaultSize) => {
    const length = normalizeSize(size)
    if (length === 0) return ''
    const bytes = new Uint8Array(length)
    randomFillSync(bytes)
    let id = ''
    for (let index = 0; index < length; index += 1) id += alphabet[bytes[index] % alphabet.length]
    return id
  }
}
