/**
 * TTS playback engine — manages Piper TTS via Web Worker with
 * sentence-by-sentence pipelining, pause/resume, and generation-based cancellation.
 */

import { stripMarkdown, splitSentences } from './tts-utils';
import { getTtsVoiceId } from './tts-models';

let currentAudio: HTMLAudioElement | null = null;
let ttsGeneration = 0;
let ttsPaused = false;
let ttsResumeResolver: (() => void) | null = null;

/** Synthesize one sentence via a TTS worker */
function synthesizeSentence(worker: Worker, text: string, voiceId: string): Promise<Blob> {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error('TTS worker timeout')), 60000);
    worker.onmessage = (e) => {
      clearTimeout(timeout);
      if (e.data.error) reject(new Error(e.data.error));
      else resolve(e.data.audio);
    };
    worker.onerror = (e) => { clearTimeout(timeout); reject(e); };
    worker.postMessage({ text, voiceId });
  });
}

/** Play an audio blob and wait for it to finish */
function playAudioBlob(blob: Blob): Promise<void> {
  return new Promise((resolve) => {
    const audio = new Audio();
    audio.src = URL.createObjectURL(blob);
    currentAudio = audio;
    audio.onended = () => { if (currentAudio === audio) currentAudio = null; resolve(); };
    audio.onerror = () => { if (currentAudio === audio) currentAudio = null; resolve(); };
    audio.play().catch(() => { if (currentAudio === audio) currentAudio = null; resolve(); });
  });
}

/** If paused, wait until resumed. Returns false if cancelled while waiting. */
function waitIfPaused(gen: number): Promise<boolean> {
  if (!ttsPaused) return Promise.resolve(true);
  return new Promise((resolve) => {
    ttsResumeResolver = () => {
      ttsResumeResolver = null;
      resolve(gen === ttsGeneration);
    };
  });
}

/** Pause TTS playback (audio + sentence loop) */
export function pauseTts() {
  ttsPaused = true;
  if (currentAudio && !currentAudio.paused) {
    currentAudio.pause();
  }
  window.speechSynthesis?.pause();
}

/** Resume TTS playback */
export function resumeTts() {
  ttsPaused = false;
  if (currentAudio && currentAudio.paused && currentAudio.src) {
    currentAudio.play();
  }
  window.speechSynthesis?.resume();
  if (ttsResumeResolver) ttsResumeResolver();
}

/** Stop TTS completely and cancel all pending */
export function stopTts() {
  ttsPaused = false;
  ttsGeneration++;
  if (ttsResumeResolver) ttsResumeResolver();
  if (currentAudio) {
    currentAudio.pause();
    currentAudio.src = '';
    currentAudio = null;
  }
  window.speechSynthesis?.cancel();
}

export function isTtsPaused() { return ttsPaused; }

/**
 * Speak text using Piper TTS with sentence-by-sentence pipelining.
 * Requires a worker factory since Worker URLs are Vite-bundler-specific.
 */
export async function speakText(
  getWorker: () => Worker,
  markdownText: string,
  lang = 'fr',
  onPlaying?: () => void,
): Promise<void> {
  stopTts();
  const gen = ttsGeneration;
  const plainText = stripMarkdown(markdownText);
  if (!plainText) return;

  const sentences = splitSentences(plainText);
  if (sentences.length === 0) return;

  const voiceId = getTtsVoiceId(lang);

  try {
    const worker = getWorker();

    let nextWav: Promise<Blob> | null = synthesizeSentence(worker, sentences[0], voiceId);

    for (let i = 0; i < sentences.length; i++) {
      if (gen !== ttsGeneration) return;

      const wav = await nextWav!;
      if (gen !== ttsGeneration) return;

      nextWav = (i + 1 < sentences.length)
        ? synthesizeSentence(worker, sentences[i + 1], voiceId)
        : null;

      if (i === 0 && onPlaying) onPlaying();

      await playAudioBlob(wav);
      if (gen !== ttsGeneration) return;

      if (ttsPaused) {
        const ok = await waitIfPaused(gen);
        if (!ok) return;
      }
    }
  } catch (err) {
    if (gen !== ttsGeneration) return;
    console.warn('Piper TTS failed, falling back to browser SpeechSynthesis:', err);
    if (window.speechSynthesis) {
      const text = sentences.join(' ');
      const utterance = new SpeechSynthesisUtterance(text.slice(0, 5000));
      utterance.lang = lang === 'fr' ? 'fr-FR' : lang === 'es' ? 'es-ES' : 'en-GB';
      utterance.rate = 1.1;
      const voices = window.speechSynthesis.getVoices();
      const voice = voices.find(v => v.lang.startsWith(lang) && v.localService)
        || voices.find(v => v.lang.startsWith(lang));
      if (voice) utterance.voice = voice;
      window.speechSynthesis.speak(utterance);
    }
  }
}
