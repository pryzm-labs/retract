const alphabet = 'useandom-26T198340PX75pxJACKVERYMINDBUSHWOLF_GQZbfghjklqvwyzrict'
const normalizeSize = (size) => Math.max(0, Number(size) | 0)

export function customAlphabet(custom, defaultSize = 21) {
  if (!custom) throw new TypeError('alphabet must not be empty')
  return (size = defaultSize) => {
    const length = normalizeSize(size)
    let id = ''
    for (let index = 0; index < length; index += 1) id += custom[(Math.random() * custom.length) | 0]
    return id
  }
}

export const nanoid = customAlphabet(alphabet)
