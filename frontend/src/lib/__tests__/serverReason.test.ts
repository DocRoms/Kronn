// KT-2003 — every code the server sends is written from the four locale
// files; the menu path comes from the navigation's own keys; a third-party
// text is quoted under a localized sentence; an unknown code falls back to
// the English sentence, never to an empty line or a raw key.
import { describe, expect, it } from 'vitest';
import {
  agentsSettingsLinkLabel, entryReasonText, fullAccessRequiredText, fullAccessSettingsPath, serverReasonText,
} from '../serverReason';
import { dictionaries } from '../i18n/testing';
import type { CatalogReasonCode } from '../../types/generated';

type Locale = keyof typeof dictionaries;
const translator = (locale: Locale) => (key: string, ...args: (string | number)[]) => {
  let text = dictionaries[locale][key as keyof typeof dictionaries[Locale]] as string | undefined ?? key;
  args.forEach((arg, index) => { text = text.split(`{${index}}`).join(String(arg)); });
  return text;
};
const LOCALES: Locale[] = ['fr', 'en', 'es', 'zh'];

const EVERY_CODE: CatalogReasonCode[] = [
  { code: 'native_full_access_required', agent: 'GitHub Copilot' },
  { code: 'catalog_not_refreshed' },
  { code: 'catalog_refresh_failed', detail: 'database is locked' },
  { code: 'catalog_lookup_failed', detail: 'database is locked' },
  { code: 'discovery_timeout' },
  { code: 'discovery_unsupported' },
  { code: 'model_disappeared', model_id: 'm-1' },
  { code: 'model_disappeared_no_replacement', model_id: 'm-1', tier: 'reasoning', agent: 'Claude Code' },
  { code: 'model_unavailable', model_id: 'm-1' },
  { code: 'model_not_found', status: 404 },
  { code: 'model_access_denied', status: 403 },
  { code: 'model_not_chat', model_id: 'flux', capabilities: ['image', 'video'] },
  { code: 'model_not_listed', model_id: 'm-1', agent: 'Codex', version: '0.99.0' },
  { code: 'agent_cli_missing', agent: 'Claude Code' },
  { code: 'agent_auth_required', agent: 'Claude Code' },
  { code: 'catalog_invalid', agent: 'Ollama' },
  { code: 'catalog_too_large', agent: 'Ollama' },
  { code: 'discovery_failed', agent: 'Codex' },
  { code: 'cli_discovery_failed', agent: 'Claude Code' },
  { code: 'catalog_http_error', agent: 'Ollama', status: 503 },
];

describe('serverReasonText (KT-2003)', () => {
  it.each(LOCALES)('writes every code from the %s locale, with no raw key and no placeholder left', locale => {
    const t = translator(locale);
    for (const code of EVERY_CODE) {
      const text = serverReasonText(t, { detail: 'ENGLISH SENTENCE', code });
      expect(text, code.code).not.toBe('ENGLISH SENTENCE');
      expect(text, code.code).not.toMatch(/serverReason\.|config\.|nav\.|disc\.tier\.|\{\d\}/);
      expect(text.length, code.code).toBeGreaterThan(0);
    }
  });

  it('names the parameters: agent, tier label, model id, status, the quoted detail', () => {
    const t = translator('fr');
    expect(serverReasonText(t, { code: EVERY_CODE[7] }))
      .toBe('Le modèle m-1 a disparu et aucun modèle disponible ne le remplace pour le mode Avancé de Claude Code.');
    expect(serverReasonText(t, { code: EVERY_CODE[10] })).toContain('HTTP 403');
    expect(serverReasonText(t, { code: EVERY_CODE[2] }))
      .toBe('L\'actualisation du catalogue des modèles a échoué : « database is locked »');
    expect(serverReasonText(t, { code: EVERY_CODE[12] })).toContain('CLI Codex v0.99.0');
  });

  it.each(LOCALES)('builds the full-access menu path from the %s navigation keys', locale => {
    const t = translator(locale);
    const dict = dictionaries[locale] as Record<string, string>;
    const path = fullAccessSettingsPath(t, 'GitHub Copilot');
    expect(path).toBe(`${dict['nav.config']} › ${dict['config.agents']} › GitHub Copilot › ${dict['config.fullAccessName']}`);
    expect(serverReasonText(t, { code: EVERY_CODE[0] })).toContain(path);
  });

  it('follows a rename of the tab without touching the sentence', () => {
    const base = translator('fr');
    const renamed = (key: string, ...args: (string | number)[]) => key === 'nav.config' ? 'Réglages' : base(key, ...args);
    expect(serverReasonText(renamed, { code: EVERY_CODE[0] })).toContain('Activez-le dans Réglages › Agents › GitHub Copilot › Accès complet');
  });

  it('quotes a third-party text as received under the sentence of its reason', () => {
    expect(serverReasonText(translator('fr'), { detail: 'error: 401 {"message":"bad key"}', reason: 'provider_error' }))
      .toBe('Le fournisseur a renvoyé une erreur : « error: 401 {"message":"bad key"} »');
    expect(serverReasonText(translator('en'), { detail: 'spawn ENOENT', reason: 'cli_missing' }))
      .toBe('The agent\'s CLI cannot be found: “spawn ENOENT”');
  });

  it('writes a sentence Kronn itself wrote during discovery, with no English quote', () => {
    const t = translator('fr');
    const missing = serverReasonText(t, {
      detail: 'Claude CLI is not installed', reason: 'cli_missing', code: { code: 'agent_cli_missing', agent: 'Claude Code' },
    });
    expect(missing).toBe('La CLI de Claude Code n\'est pas installée ou ne peut pas être lancée.');
    expect(missing).not.toMatch(/«|not installed/);
    expect(serverReasonText(t, { code: { code: 'agent_auth_required', agent: 'Claude Code' } }))
      .toBe('Connectez-vous à la CLI de Claude Code pour que Kronn puisse lister ses modèles.');
    expect(serverReasonText(t, { code: { code: 'catalog_http_error', agent: 'Ollama', status: 503 } }))
      .toBe('Ollama a répondu HTTP 503 à la demande de la liste des modèles.');
    expect(serverReasonText(translator('zh'), { code: { code: 'cli_discovery_failed', agent: 'Claude Code' } }))
      .toContain('--safe-mode');
  });

  it('keeps both halves of « not found or access denied » in every locale', () => {
    const halves: Record<Locale, RegExp> = {
      en: /Not found or access denied \(HTTP 403\)/,
      fr: /Introuvable ou accès refusé \(HTTP 403\)/,
      es: /No encontrado o acceso denegado \(HTTP 403\)/,
      zh: /未找到或访问被拒绝（HTTP 403）/,
    };
    for (const locale of LOCALES) {
      expect(serverReasonText(translator(locale), { code: { code: 'model_not_found', status: 403 } })).toMatch(halves[locale]);
    }
  });

  it.each(LOCALES)('says « full access required » in one %s sentence, server refusal and pre-launch warning alike', locale => {
    const t = translator(locale);
    const dict = dictionaries[locale] as Record<string, string>;
    expect(fullAccessRequiredText(t, 'GitHub Copilot')).toBe(serverReasonText(t, { code: EVERY_CODE[0] }));
    expect(agentsSettingsLinkLabel(t)).toContain(`${dict['nav.config']} › ${dict['config.agents']}`);
    expect(agentsSettingsLinkLabel(t)).not.toMatch(/\{\d\}/);
    if (locale === 'zh') {
      // The word for « agent » in that sentence is the Settings section's own.
      expect(fullAccessRequiredText(t, 'GitHub Copilot')).toMatch(new RegExp(`选择其他${dict['config.agents']}。$`));
    }
  });

  it('follows a rename of the tab in the link that opens the Agents section', () => {
    const base = translator('fr');
    const renamed = (key: string, ...args: (string | number)[]) => key === 'nav.config' ? 'Réglages' : base(key, ...args);
    expect(agentsSettingsLinkLabel(renamed)).toBe('Ouvrir Réglages › Agents');
    expect(fullAccessRequiredText(renamed, 'OpenCode')).toContain('Activez-le dans Réglages › Agents › OpenCode › Accès complet');
  });

  it('falls back to the English sentence for an unknown code, and says nothing when the server said nothing', () => {
    const t = translator('fr');
    const unknown = { code: 'a_newer_reason' } as unknown as CatalogReasonCode;
    expect(serverReasonText(t, { detail: 'The English sentence.', code: unknown })).toBe('The English sentence.');
    expect(serverReasonText(t, null)).toBe('');
    expect(serverReasonText(t, { detail: null, reason: 'timeout' })).toBe('');
  });

  it('reads a catalogue entry’s code, then its quoted detail', () => {
    const t = translator('es');
    expect(entryReasonText(t, { unavailable_code: { code: 'model_access_denied', status: 401 }, unavailable_detail: 'Access denied (HTTP 401)…', unavailable_reason: 'access_denied' }))
      .toMatch(/^Acceso denegado \(HTTP 401\)/);
    expect(entryReasonText(t, { unavailable_detail: 'upstream 502', unavailable_reason: 'not_found' }))
      .toBe('Modelo no encontrado: «upstream 502»');
  });
});
