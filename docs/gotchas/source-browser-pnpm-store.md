# Source-browser pnpm cache exclusion

The Code tree skips `.pnpm-store` directories at every depth before consuming
the global source-file budget. The regression test starts one slot below that
budget and proves that `site/en.html` remains visible when a cache exists under
`frontend/`. [src: file: backend/src/api/ai_docs.rs:397-429#is_skipped_source_dir] [src: file: backend/src/api/ai_docs.rs:981-984#is_skipped_source_dir] [src: file: backend/src/api/ai_docs.rs:1869-1889#source_tree_skips_nested_pnpm_store_before_applying_file_cap]
