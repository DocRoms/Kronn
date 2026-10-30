// KT-1490 — a Quick Exec line shown to the human who activates it. Joining the
// argv with spaces hides where one argument ends: ["purge", "--key", "{{id}}"]
// and ["purge --key", "{{id}}"] would read the same yet run differently. Each
// word is quoted unless it is plain, so the display and the payload agree.

const PLAIN_WORD = /^[A-Za-z0-9_\-./:=@%+,{}]+$/;

/** One argv word, single-quoted (POSIX style) unless it is plain. */
export function quoteWord(word: string): string {
  if (word !== '' && PLAIN_WORD.test(word)) return word;
  return `'${word.replace(/'/g, "'\\''")}'`;
}

/** The program and its arguments, one unambiguous word each. */
export function commandLine(command: string, args: readonly string[]): string {
  return [command, ...args].map(quoteWord).join(' ');
}
