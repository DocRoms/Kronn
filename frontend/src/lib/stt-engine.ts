/**
 * STT recording engine — manages microphone capture, audio decoding,
 * and communication with the Whisper Web Worker.
 */

import { getSttModelId } from './stt-models';

/** Whisper language codes */
export const WHISPER_LANGS: Record<string, string> = {
  fr: 'french',
  en: 'english',
  es: 'spanish',
};

/** Convert AudioBuffer to mono Float32Array at 16kHz (Whisper requirement) */
export function audioBufferToFloat32(buffer: AudioBuffer): Float32Array {
  const mono = buffer.numberOfChannels > 1
    ? new Float32Array(buffer.length).map((_, i) => {
        let sum = 0;
        for (let ch = 0; ch < buffer.numberOfChannels; ch++) sum += buffer.getChannelData(ch)[i];
        return sum / buffer.numberOfChannels;
      })
    : buffer.getChannelData(0);

  if (buffer.sampleRate === 16000) return mono;
  const ratio = buffer.sampleRate / 16000;
  const newLen = Math.round(mono.length / ratio);
  const resampled = new Float32Array(newLen);
  for (let i = 0; i < newLen; i++) {
    resampled[i] = mono[Math.round(i * ratio)];
  }
  return resampled;
}

/** Send audio to the STT worker and wait for transcription */
// Safe: TTS sentences are processed sequentially (awaited), and STT calls are one-at-a-time.
// If concurrent calls are ever needed, add a message ID protocol.
export function transcribeAudio(
  worker: Worker,
  audio: Float32Array,
  lang: string,
): Promise<string> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('STT timeout')), 120000);
    worker.onmessage = (e) => {
      if (e.data.text !== undefined) {
        clearTimeout(timeout);
        resolve(e.data.text);
      } else if (e.data.error) {
        clearTimeout(timeout);
        reject(new Error(e.data.error));
      }
      // status messages are ignored
    };
    worker.onerror = (e) => { clearTimeout(timeout); reject(e); };
    worker.postMessage({
      audio,
      language: WHISPER_LANGS[lang] || 'french',
      model: getSttModelId(),
    });
  });
}
