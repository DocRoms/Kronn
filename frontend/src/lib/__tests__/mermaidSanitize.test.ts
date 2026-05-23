import { describe, it, expect } from 'vitest';
import { sanitizeMermaidSource } from '../mermaidSanitize';

describe('sanitizeMermaidSource', () => {
  it('renames a reserved-keyword alias (Alt) in declaration + every reference', () => {
    const src = [
      'sequenceDiagram',
      '    participant Caddy as Caddy',
      '    participant Alt as AlternateLocaleSubscriber',
      '    Caddy->>Alt: kernel.request default prio',
      '    Alt->>Router: generate route per locale',
      '    Alt-->>Caddy: set _alternates and _canonical',
    ].join('\n');

    const out = sanitizeMermaidSource(src);

    // Declaration alias renamed, but the `as <label>` is untouched.
    expect(out).toContain('participant Alt_ as AlternateLocaleSubscriber');
    // Structural references renamed.
    expect(out).toContain('Caddy->>Alt_:');
    expect(out).toContain('Alt_->>Router:');
    expect(out).toContain('Alt_-->>Caddy:');
    // No bare `Alt` left as a participant token (the keyword collision is gone).
    expect(out).not.toMatch(/->>Alt\b(?!_)/);
    expect(out).not.toMatch(/participant Alt /);
  });

  it('never touches message text after the colon', () => {
    const src = [
      'sequenceDiagram',
      '    participant Alt as X',
      '    A->>Alt: the Alt subscriber sets alternates',
    ].join('\n');

    const out = sanitizeMermaidSource(src);
    // The token before the colon is renamed; the prose after it is verbatim.
    expect(out).toContain('A->>Alt_: the Alt subscriber sets alternates');
  });

  it('does not corrupt an `as` label that contains the alias word', () => {
    const src = [
      'sequenceDiagram',
      '    participant Note as Note Service',
      '    A->>Note: ping',
    ].join('\n');

    const out = sanitizeMermaidSource(src);
    expect(out).toContain('participant Note_ as Note Service'); // label intact
    expect(out).toContain('A->>Note_: ping');
  });

  it('renames in Note/activate structural positions too', () => {
    const src = [
      'sequenceDiagram',
      '    participant Loop as L',
      '    activate Loop',
      '    Note over Loop,A: hello',
      '    deactivate Loop',
    ].join('\n');

    const out = sanitizeMermaidSource(src);
    expect(out).toContain('activate Loop_');
    expect(out).toContain('Note over Loop_,A: hello');
    expect(out).toContain('deactivate Loop_');
  });

  it('is a no-op when no alias collides with a keyword', () => {
    const src = [
      'sequenceDiagram',
      '    participant AltLoc as AlternateLocaleSubscriber',
      '    Caddy->>AltLoc: msg',
    ].join('\n');
    expect(sanitizeMermaidSource(src)).toBe(src);
  });

  it('is a no-op for non-sequence diagrams', () => {
    const flow = 'flowchart TD\n  A[Start] --> end[Finish]';
    expect(sanitizeMermaidSource(flow)).toBe(flow);
  });

  it('handles multiple colliding aliases and avoids name clashes', () => {
    const src = [
      'sequenceDiagram',
      '    participant End as EndpointService',
      '    participant Par as ParserService',
      '    End->>Par: parse',
      '    Par-->>End: done',
    ].join('\n');

    const out = sanitizeMermaidSource(src);
    expect(out).toContain('participant End_ as EndpointService');
    expect(out).toContain('participant Par_ as ParserService');
    expect(out).toContain('End_->>Par_: parse');
    expect(out).toContain('Par_-->>End_: done');
  });
});
