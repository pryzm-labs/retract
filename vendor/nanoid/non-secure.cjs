'use strict'

const alphabet = 'useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict'

function normalizeSize(size) {
  const normalized = Number(size) | 0
  return normalized > 0 ? normalized : 0
}

function customAlphabet(custom, defaultSize = 21) {
  if (!custom) throw new TypeError('alphabet must not be empty')
  return (size = defaultSize) => {
    const length = normalizeSize(size)
    let id = ''
    for (let index = 0; index < length; index += 1) id += custom[(Math.random() * custom.length) | 0]
    return id
  }
}

const nanoid = customAlphabet(alphabet)
module.exports = { nanoid, customAlphabet }
