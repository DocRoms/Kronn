# Source-code HTML preview

The Project Code viewer offers an isolated preview only for `.html` and `.htm`
paths, case-insensitively. It uses an iframe with an empty `sandbox` attribute
and a document shell that applies a restrictive Content Security Policy before
repository markup. The static-preview sanitizer removes scripts, forms,
navigation URLs, refresh metadata, and resource-bearing elements. Inline CSS
and data URL fonts or images remain suitable for self-contained previews.

The viewer switches back to source when a non-HTML file is selected, and keeps
its ordinary loading and error state rather than rendering a stale preview.

[src: file: frontend/src/lib/html-preview.ts:1-28]
[src: file: frontend/src/components/SourceCodeViewer.tsx:295-301]
[src: file: frontend/src/components/SourceCodeViewer.tsx:533-561]
