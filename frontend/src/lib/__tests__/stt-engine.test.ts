import { describe, it, expect } from 'vitest';
import { WHISPER_LANGS, audioBufferToFloat32 } from '../stt-engine';

describe('WHISPER_LANGS', () => {
  it('maps fr, en, es to whisper language names', () => {
    expect(WHISPER_LANGS.fr).toBe('french');
    expect(WHISPER_LANGS.en).toBe('english');
    expect(WHISPER_LANGS.es).toBe('spanish');
  });

  it('has exactly 3 language entries', () => {
    expect(Object.keys(WHISPER_LANGS)).toHaveLength(3);
  });
});

describe('audioBufferToFloat32', () => {
  // Helper to create a mock AudioBuffer
  function mockAudioBuffer(opts: {
    sampleRate: number;
    channels: Float32Array[];
  }): AudioBuffer {
    const length = opts.channels[0].length;
    return {
      sampleRate: opts.sampleRate,
      length,
      numberOfChannels: opts.channels.length,
      duration: length / opts.sampleRate,
      getChannelData: (ch: number) => opts.channels[ch],
      copyFromChannel: () => {},
      copyToChannel: () => {},
    } as unknown as AudioBuffer;
  }

  it('returns mono channel directly at 16kHz', () => {
    const data = new Float32Array([0.1, 0.2, 0.3, 0.4]);
    const buf = mockAudioBuffer({ sampleRate: 16000, channels: [data] });
    const result = audioBufferToFloat32(buf);
    expect(result).toBe(data); // same reference, no copy
  });

  it('downsamples from 48kHz to 16kHz', () => {
    // 48kHz → 16kHz = 3:1 ratio
    const data = new Float32Array(48);
    for (let i = 0; i < 48; i++) data[i] = i / 48;
    const buf = mockAudioBuffer({ sampleRate: 48000, channels: [data] });
    const result = audioBufferToFloat32(buf);
    expect(result.length).toBe(16); // 48 / 3 = 16
  });

  it('mixes stereo to mono', () => {
    const left  = new Float32Array([1.0, 0.0, 0.5]);
    const right = new Float32Array([0.0, 1.0, 0.5]);
    const buf = mockAudioBuffer({ sampleRate: 16000, channels: [left, right] });
    const result = audioBufferToFloat32(buf);
    expect(result.length).toBe(3);
    expect(result[0]).toBeCloseTo(0.5); // (1+0)/2
    expect(result[1]).toBeCloseTo(0.5); // (0+1)/2
    expect(result[2]).toBeCloseTo(0.5); // (0.5+0.5)/2
  });

  it('mixes stereo and resamples combined', () => {
    const left  = new Float32Array(48).fill(0.8);
    const right = new Float32Array(48).fill(0.2);
    const buf = mockAudioBuffer({ sampleRate: 48000, channels: [left, right] });
    const result = audioBufferToFloat32(buf);
    expect(result.length).toBe(16);
    // Each sample should be (0.8+0.2)/2 = 0.5
    for (const v of result) {
      expect(v).toBeCloseTo(0.5);
    }
  });
});
