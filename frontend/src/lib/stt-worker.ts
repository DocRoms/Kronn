/**
 * STT Web Worker — runs Whisper inference off the main thread.
 * Receives { audio: Float32Array, language: string, model: string } messages.
 * Returns { text: string } or { error: string }.
 * Sends { status: string } for progress updates.
 */

import { pipeline, type AutomaticSpeechRecognitionPipeline, type AutomaticSpeechRecognitionOutput } from '@huggingface/transformers';

let transcriber: AutomaticSpeechRecognitionPipeline | null = null;
let loadedModel: string | null = null;

async function getTranscriber(model: string) {
  if (!transcriber || loadedModel !== model) {
    transcriber = null;
    loadedModel = null;
    self.postMessage({ status: 'loading' });
    // pipeline() generic union is too complex for TS — cast the result to the specific pipeline type
    transcriber = await pipeline(
      'automatic-speech-recognition',
      model,
      { dtype: 'q8' },
    ) as unknown as AutomaticSpeechRecognitionPipeline;
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
    const text = Array.isArray(result)
      ? result.map((r: AutomaticSpeechRecognitionOutput) => r.text).join(' ')
      : result.text;
    self.postMessage({ text: text.trim() });
  } catch (err) {
    self.postMessage({ error: String(err) });
  }
};
