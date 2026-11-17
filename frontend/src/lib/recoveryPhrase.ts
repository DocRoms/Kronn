// KT-1686 — a recovery phrase Kronn writes for the owner: 25 characters drawn
// from the system's random source (about 124 bits), in groups a person can
// copy. No look-alike characters (0/o, 1/l/i).
const SYMBOLS = 'abcdefghjkmnpqrstuvwxyz23456789';
const GROUPS = 5;
const GROUP_LEN = 5;

export function generateRecoveryPhrase(): string {
  const drawn: string[] = [];
  // Rejection sampling: a byte past the last full multiple of the alphabet is
  // drawn again, so every symbol is as likely as another.
  const limit = 256 - (256 % SYMBOLS.length);
  const bytes = new Uint8Array(1);
  while (drawn.length < GROUPS * GROUP_LEN) {
    crypto.getRandomValues(bytes);
    if (bytes[0] < limit) drawn.push(SYMBOLS[bytes[0] % SYMBOLS.length]);
  }
  const groups: string[] = [];
  for (let i = 0; i < GROUPS; i += 1) groups.push(drawn.slice(i * GROUP_LEN, (i + 1) * GROUP_LEN).join(''));
  return groups.join('-');
}
