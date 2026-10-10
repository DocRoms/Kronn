// Keeps secret values typed in a form out of an assistant conversation, which
// is now kept (KT-1111). Mirrors the backend `core::secret_scrub` forms.
import { redactSecrets } from './bug-report';

export const SECRET_MASK = '***';

/** Shorter secrets are masked only as whole tokens, like the backend. */
const MIN_ANYWHERE_LEN = 8;

const SENSITIVE_NAME_RX = /(auth|token|secret|passw|pwd|api[-_]?key|apikey|access[-_]?key|private[-_]?key|client[-_]?secret|cookie|session|signature|credential|bearer)/i;

/** Header or query name whose value is a credential. */
export function looksSensitiveName(name: string): boolean {
  return SENSITIVE_NAME_RX.test(name);
}

const NON_SENSITIVE_HEADERS = new Set([
  'accept', 'accept-language', 'accept-encoding', 'content-type', 'cache-control', 'user-agent',
]);

/** Header whose literal value is known to carry no credential (KT-1041). */
export function isKnownNonSensitiveHeader(name: string): boolean {
  const n = name.trim().toLowerCase();
  return NON_SENSITIVE_HEADERS.has(n) || n === 'version' || n.endsWith('-version');
}

/** `{{var}}` / `${ENV.X}` placeholders are references, never the secret itself. */
export function isPlaceholder(value: string): boolean {
  const v = value.trim();
  return /^\{\{[^{}]+\}\}$/.test(v) || /^\$\{[^{}]+\}$/.test(v);
}

function utf8Bytes(value: string): Uint8Array {
  return new TextEncoder().encode(value);
}

function base64(value: string, urlSafe: boolean, pad: boolean): string {
  let binary = '';
  for (const byte of utf8Bytes(value)) binary += String.fromCharCode(byte);
  let out = btoa(binary);
  if (urlSafe) out = out.replace(/\+/g, '-').replace(/\//g, '_');
  if (!pad) out = out.replace(/=+$/, '');
  return out;
}

function percentEncode(value: string, spaceAsPlus: boolean, lowerHex: boolean): string {
  let out = '';
  for (const byte of utf8Bytes(value)) {
    const ch = String.fromCharCode(byte);
    if (/[A-Za-z0-9\-._~]/.test(ch)) out += ch;
    else if (byte === 0x20 && spaceAsPlus) out += '+';
    else {
      const hex = byte.toString(16).padStart(2, '0');
      out += `%${lowerHex ? hex : hex.toUpperCase()}`;
    }
  }
  return out;
}

/** Every wire form a secret may be echoed in. */
export function secretForms(secret: string): string[] {
  const raw = secret.trim();
  if (!raw) return [];
  const json = JSON.stringify(raw).slice(1, -1);
  const hex = Array.from(utf8Bytes(raw), byte => byte.toString(16).padStart(2, '0')).join('');
  const forms = [
    raw,
    hex,
    json,
    percentEncode(raw, false, false),
    percentEncode(raw, false, true),
    percentEncode(raw, true, false),
    percentEncode(raw, true, true),
  ];
  // A base64 form of a very short secret is itself short and generic.
  if (raw.length >= 4) {
    forms.push(base64(raw, false, true), base64(raw, false, false), base64(raw, true, true), base64(raw, true, false));
  }
  return Array.from(new Set(forms.filter(Boolean)));
}

function escapeRegExp(value: string): string {
  return value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&');
}

/** Masks every form of every secret in `text`. */
export function scrubSecrets(text: string, secrets: Iterable<string>): string {
  const forms = new Set<string>();
  for (const secret of secrets) for (const form of secretForms(secret)) forms.add(form);
  if (forms.size === 0 || !text) return text;
  let out = text;
  for (const form of Array.from(forms).sort((a, b) => b.length - a.length)) {
    const pattern = escapeRegExp(form);
    out = form.length >= MIN_ANYWHERE_LEN
      ? out.replace(new RegExp(pattern, 'gi'), SECRET_MASK)
      : out.replace(new RegExp(`(^|[^A-Za-z0-9_])${pattern}(?![A-Za-z0-9_])`, 'g'), `$1${SECRET_MASK}`);
  }
  return out;
}

/** What may enter a kept conversation: known form secrets and common token
 *  shapes (Bearer, `sk-…`, JSON password fields) are masked. */
export function sanitizeForAssistant(text: string, secrets: Iterable<string>): string {
  return redactSecrets(scrubSecrets(text, secrets));
}

/** Values of a name→value map whose name is sensitive, placeholders excluded. */
export function sensitiveValues(map: Record<string, string> | null | undefined): string[] {
  if (!map) return [];
  return Object.entries(map)
    .filter(([name, value]) => looksSensitiveName(name) && typeof value === 'string' && value.trim() && !isPlaceholder(value))
    .map(([, value]) => value);
}

/** The same map with sensitive values replaced by the mask. */
export function maskSensitiveValues(map: Record<string, string>): Record<string, string> {
  return Object.fromEntries(Object.entries(map).map(([name, value]) => [
    name,
    looksSensitiveName(name) && typeof value === 'string' && value.trim() && !isPlaceholder(value) ? SECRET_MASK : value,
  ]));
}
