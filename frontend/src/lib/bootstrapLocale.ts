import {
  getUILocale,
  loadLocale,
  setUILocale,
  type TranslationDict,
  type UILocale,
} from './i18n';

type LocaleLoader = (locale: UILocale) => Promise<TranslationDict>;

export async function loadInitialLocale(
  preferred = getUILocale(),
  loader: LocaleLoader = loadLocale,
  persist: (locale: UILocale) => void = setUILocale,
): Promise<UILocale> {
  try {
    await loader(preferred);
    return preferred;
  } catch (primaryError) {
    if (preferred === 'en') throw primaryError;
    await loader('en');
    persist('en');
    return 'en';
  }
}

export function renderBootstrapFailure(
  root: HTMLElement,
  onRetry = () => window.location.reload(),
  detailMessage = 'A language resource is unavailable. Check the connection, then retry.',
) {
  root.replaceChildren();
  const panel = document.createElement('main');
  panel.setAttribute('role', 'alert');
  panel.className = 'bootstrap-error';

  const title = document.createElement('h1');
  title.textContent = 'Kronn could not start';
  const detail = document.createElement('p');
  detail.textContent = detailMessage;
  const retry = document.createElement('button');
  retry.type = 'button';
  retry.className = 'btn btn-primary';
  retry.textContent = 'Retry';
  retry.addEventListener('click', onRetry);

  panel.append(title, detail, retry);
  root.append(panel);
}
