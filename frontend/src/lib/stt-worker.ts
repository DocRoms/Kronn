/**
 * STT Web Worker — runs Whisper inference off the main thread.
 * Receives { audio: Float32Array, language: string, model: string } messages.
 * Returns { text: string } or { error: string }.
 * Sends { status: string } for progress updates.
 */

import { pipeline } from '@huggingface/transformers';

// eslint-disable-next-line @typescript-eslint/no-explicit-any
let transcriber: any = null;
let loadedModel: string | null = null;

async function getTranscriber(model: string) {
  if (!transcriber || loadedModel !== model) {
    transcriber = null;
    loadedModel = null;
    self.postMessage({ status: 'loading' });
    transcriber = await (pipeline as any)(
      'automatic-speech-recognition',
      model,
      { dtype: 'q8' },
    );
    loadedModel = model;
    self.postMessage({ status: 'ready' });
  }
  return transcriber;
}

self.onmessage = async (event: MessageEvent<{ audio: Float32Array; language: string; model: string }>) => {
  const { audio, language, model } = event.data;
  try {
    const t = await getTranscriber(model);
    self.postMessage({ status: 'transcribing' });
    const result = await t(audio, {
      language,
      task: 'transcribe',
    });
    const text = Array.isArray(result) ? result.map((r: any) => r.text).join(' ') : result.text;
    self.postMessage({ text: text.trim() });
  } catch (err) {
    self.postMessage({ error: String(err) });
  }
};
