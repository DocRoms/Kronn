/**
 * TTS Web Worker — runs Piper TTS inference off the main thread.
 * Receives { text, voiceId } messages, returns { audio: Blob } or { error: string }.
 */

import * as tts from '@diffusionstudio/vits-web';
import type { VoiceId } from '@diffusionstudio/vits-web';

self.onmessage = async (event: MessageEvent<{ text: string; voiceId: string }>) => {
  const { text, voiceId } = event.data;
  try {
    const wav = await tts.predict({
      text,
      voiceId: voiceId as VoiceId,
    });
    self.postMessage({ audio: wav });
  } catch (err) {
    self.postMessage({ error: String(err) });
  }
};
