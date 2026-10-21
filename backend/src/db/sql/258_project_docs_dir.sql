-- KT-263: where the project keeps its AI doc, relative to its root. NULL falls
-- back to the global default, then to the historical detection order.
ALTER TABLE projects ADD COLUMN docs_dir TEXT;
