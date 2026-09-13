// KT-619 — publishing a steering card in your own name, from the composer.
//
// A `kronn-important` fence typed by a human publishes nothing on its own: the
// card needs an enrolled credential and a single-use proof bound to this room
// and this exact body. Without this strip the whole human path existed only on
// the API — a reviewer's words, and they were right: enrolling a grant in
// Settings and then reaching for curl is not a path a person has.
//
// The credential is typed here and held for as long as the page is open. Not
// localStorage, not a URL, not a query string: reloading asks again, which is
// the point — a secret that survives a reload is a secret sitting somewhere.

import { KeyRound } from 'lucide-react';
import './ImportantMessageCard.css';

export interface ImportantPublicationStripProps {
  grant: string;
  onGrantChange: (value: string) => void;
  t: (key: string, ...args: (string | number)[]) => string;
}

export function ImportantPublicationStrip({
  grant,
  onGrantChange,
  t,
}: ImportantPublicationStripProps) {
  return (
    <details className="disc-important-publish">
      <summary>
        <KeyRound size={14} aria-hidden="true" /> {t('disc.important.publishAs')}
      </summary>
      <div className="disc-important-publish-body">
        <label htmlFor="important-publish-grant">{t('disc.important.grantLabel')}</label>
        <input
          id="important-publish-grant"
          type="password"
          autoComplete="off"
          value={grant}
          onChange={(event) => onGrantChange(event.target.value)}
          placeholder={t('disc.important.grantPlaceholder')}
        />
        <p className="settings-hint">{t('disc.important.grantHint')}</p>
      </div>
    </details>
  );
}
