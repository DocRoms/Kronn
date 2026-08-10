// Test/dev-only static aggregate. Production code must import from `../i18n`
// so Vite keeps each locale in a separate lazy chunk.
import fr from './locales/fr';
import en from './locales/en';
import es from './locales/es';
import zh from './locales/zh';

export const dictionaries = { fr, en, es, zh } as const;
