'use strict'

const { randomFillSync } = require('node:crypto')

const alphabet = 'useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict'

function normalizeSize(size) {
  const normalized = Number(size) | 0
  return normalized > 0 ? normalized : 0
}

function nanoid(size = 21) {
  const length = normalizeSize(size)
  if (length === 0) return ''
  const bytes = Buffer.allocUnsafe(length)
  randomFillSync(bytes)
  let id = ''
  for (let index = 0; index < length; index += 1) id += alphabet[bytes[index] & 63]
  return id
}

function customAlphabet(custom, defaultSize = 21) {
  if (!custom || custom.length > 256) throw new TypeError('alphabet must contain 1 to 256 symbols')
  return (size = defaultSize) => {
    const length = normalizeSize(size)
    if (length === 0) return ''
    const bytes = Buffer.allocUnsafe(length)
    randomFillSync(bytes)
    let id = ''
    for (let index = 0; index < length; index += 1) id += custom[bytes[index] % custom.length]
    return id
  }
}

module.exports = { nanoid, customAlphabet, urlAlphabet: alphabet }
