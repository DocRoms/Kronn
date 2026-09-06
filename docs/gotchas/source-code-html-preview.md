# Source-code HTML preview

The Project Code viewer offers an isolated preview only for `.html` and `.htm`
paths, case-insensitively. It uses an iframe with an empty `sandbox` attribute
and a document shell that applies a restrictive Content Security Policy before
repository markup. A detached `DOMParser` pass removes scripts, forms,
navigation URLs, refresh metadata, and resource-bearing elements before the
source is serialized into that shell. Inline CSS and `data:image/...` sources
remain suitable for self-contained previews.

Template contents (including declarative shadow roots) and SVG mutation/
animation elements are removed entirely. A traversal of `querySelectorAll('*')`
does not inspect `template.content`; SVG SMIL can restore an `href` after URL
attributes have been removed. Browser regressions therefore click the actual
SVG anchor, not only elements with an ARIA link role, and keep network
monitoring active through parsing, rendering and clicking.

The viewer switches back to source when a non-HTML file is selected, and keeps
its ordinary loading and error state rather than rendering a stale preview.

[src: file: frontend/src/lib/html-preview.ts:1-53]
[src: file: frontend/src/components/SourceCodeViewer.tsx:350-380]
[src: file: frontend/src/components/SourceCodeViewer.tsx:510-514]
