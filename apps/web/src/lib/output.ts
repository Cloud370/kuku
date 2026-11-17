export function visibleOutput(text: string) {
  return text.replace(/\p{Cc}/gu, (character) => {
    if (character === "\n" || character === "\t") return character;
    const code = character.charCodeAt(0);
    if (code < 32) return String.fromCharCode(0x2400 + code);
    if (code === 127) return "\u2421";
    return `\\u${code.toString(16).padStart(4, "0")}`;
  });
}
