export interface TtsVoice {
  id: string;
  label: string;
  gender: 'M' | 'F';
  quality: string;
}

export interface TtsLangVoices {
  label: string;
  voices: TtsVoice[];
  default: string;
}

export const TTS_VOICES: Record<string, TtsLangVoices> = {
  fr: {
    label: 'Français',
    default: 'fr_FR-upmc-medium',
    voices: [
      { id: 'fr_FR-upmc-medium',   label: 'UPMC',  gender: 'M', quality: 'medium' },
      { id: 'fr_FR-siwis-medium',  label: 'Siwis', gender: 'F', quality: 'medium' },
      { id: 'fr_FR-tom-medium',    label: 'Tom',   gender: 'M', quality: 'medium' },
    ],
  },
  en: {
    label: 'English',
    default: 'en_US-hfc_female-medium',
    voices: [
      { id: 'en_US-hfc_female-medium', label: 'HFC Female', gender: 'F', quality: 'medium' },
      { id: 'en_US-hfc_male-medium',   label: 'HFC Male',   gender: 'M', quality: 'medium' },
      { id: 'en_US-lessac-medium',      label: 'Lessac',     gender: 'F', quality: 'medium' },
    ],
  },
  es: {
    label: 'Español',
    default: 'es_ES-sharvard-medium',
    voices: [
      { id: 'es_ES-sharvard-medium', label: 'Sharvard', gender: 'M', quality: 'medium' },
      { id: 'es_ES-davefx-medium',   label: 'DaveFX',   gender: 'M', quality: 'medium' },
      { id: 'es_MX-ald-medium',      label: 'Ald (MX)', gender: 'F', quality: 'medium' },
    ],
  },
};

/** Get the selected voice ID for a language, falling back to the language default */
export function getTtsVoiceId(lang: string): string {
  try {
    const stored = localStorage.getItem(`kronn:ttsVoice:${lang}`);
    const langVoices = TTS_VOICES[lang];
    if (stored && langVoices?.voices.some(v => v.id === stored)) return stored;
  } catch { /* ignore */ }
  return TTS_VOICES[lang]?.default ?? TTS_VOICES.fr.default;
}

/** Store the selected voice ID for a language */
export function setTtsVoiceId(lang: string, voiceId: string) {
  localStorage.setItem(`kronn:ttsVoice:${lang}`, voiceId);
}
