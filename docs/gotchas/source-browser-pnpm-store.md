# Source-browser pnpm cache exclusion

The Code tree skips `.pnpm-store` directories at every depth before consuming
the global source-file budget. The regression test starts one slot below that
budget and proves that `site/en.html` remains visible when a cache exists under
`frontend/`. [src: file: backend/src/api/ai_docs.rs:395-429] [src: file: backend/src/api/ai_docs.rs:1522-1542]
