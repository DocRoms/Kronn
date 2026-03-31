//! Context file text extraction and prompt building.
//!
//! Extracts readable text from uploaded files (text, xlsx, docx, pptx, pdf)
//! and builds the `=== CONTEXT FILES ===` prompt section.

use anyhow::{Result, bail};

/// Max extracted text size (500KB)
const MAX_EXTRACTED_SIZE: usize = 512_000;
/// Max context files per discussion
pub const MAX_FILES_PER_DISCUSSION: usize = 20;

/// Image extensions (saved to disk, referenced by path in prompt)
const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "svg", "bmp", "tiff", "ico",
];

/// Text file extensions (stored as-is)
const TEXT_EXTENSIONS: &[&str] = &[
    "txt", "md", "json", "csv", "yaml", "yml", "toml",
    "sql", "xml", "html", "htm", "css", "js", "ts",
    "jsx", "tsx", "py", "rs", "go", "java", "c", "cpp",
    "h", "hpp", "rb", "sh", "bash", "zsh", "fish",
    "log", "env", "ini", "cfg", "conf", "properties",
    "swift", "kt", "kts", "scala", "clj", "ex", "exs",
    "vue", "svelte", "astro", "php", "r", "jl", "lua",
    "tf", "hcl", "dockerfile", "makefile", "cmake",
    "gitignore", "editorconfig", "prettierrc", "eslintrc",
];

/// Result of processing an uploaded file.
pub enum ExtractedContent {
    /// Text content (code, documents) — stored in DB
    Text(String),
    /// Image — must be saved to disk, agents reference by path
    Image { data: Vec<u8>, ext: String },
}

/// Check if a file is an image based on extension.
pub fn is_image(filename: &str) -> bool {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    IMAGE_EXTENSIONS.contains(&ext.as_str())
}

/// Save image bytes to a target directory (project work_dir or temp).
/// Returns the absolute path of the saved file.
pub fn save_image_to_dir(dir: &std::path::Path, id: &str, filename: &str, _ext: &str, data: &[u8]) -> Result<String> {
    // Save in .kronn/context-files/ within the target directory
    let ctx_dir = dir.join(".kronn").join("context-files");
    std::fs::create_dir_all(&ctx_dir)?;
    // Ensure .kronn/context-files/ is gitignored
    if let Some(dir_str) = dir.to_str() {
        crate::core::mcp_scanner::ensure_gitignore_public(dir_str, ".kronn/context-files/");
    }
    // Use original filename for readability (agent sees a meaningful name)
    let safe_name = format!("{}_{}", &id[..8], filename.replace(['/', '\\', ' '], "_"));
    let path = ctx_dir.join(&safe_name);
    std::fs::write(&path, data)?;
    Ok(path.to_string_lossy().to_string())
}

/// Save image bytes to disk. Returns the absolute path.
/// Fallback when no project work_dir is available.
pub fn save_image_to_disk(id: &str, ext: &str, data: &[u8]) -> Result<String> {
    let dir = crate::core::config::config_dir()
        .unwrap_or_else(|_| std::path::PathBuf::from("/tmp"))
        .join("context-files");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.{}", id, ext));
    std::fs::write(&path, data)?;
    Ok(path.to_string_lossy().to_string())
}

/// Delete an image file from disk.
pub fn delete_image_from_disk(disk_path: &str) {
    let _ = std::fs::remove_file(disk_path);
}

/// Extract content from a file's raw bytes.
/// Returns Text for documents, Image for images, error for unsupported.
pub fn extract_content(filename: &str, data: &[u8]) -> Result<ExtractedContent> {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();

    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        if data.len() > 10 * 1024 * 1024 {
            bail!("Image exceeds 10MB limit ({} MB)", data.len() / (1024 * 1024));
        }
        return Ok(ExtractedContent::Image { data: data.to_vec(), ext });
    }

    let text = extract_text(filename, data)?;
    Ok(ExtractedContent::Text(text))
}

/// Extract text content from a file's raw bytes.
/// Returns the extracted text or an error for unsupported/binary files.
pub fn extract_text(filename: &str, data: &[u8]) -> Result<String> {
    let ext = filename.rsplit('.')
        .next()
        .unwrap_or("")
        .to_lowercase();

    // Also match extensionless files by known names
    let basename = filename.rsplit('/').next()
        .and_then(|n| n.rsplit('\\').next())
        .unwrap_or(filename)
        .to_lowercase();

    let text = if TEXT_EXTENSIONS.contains(&ext.as_str())
        || TEXT_EXTENSIONS.contains(&basename.as_str())
    {
        String::from_utf8(data.to_vec())
            .map_err(|_| anyhow::anyhow!("File is not valid UTF-8 text"))?
    } else if ext == "xlsx" || ext == "xls" {
        extract_xlsx(data)?
    } else if ext == "docx" {
        extract_docx(data)?
    } else if ext == "pptx" {
        extract_pptx(data)?
    } else if ext == "pdf" {
        extract_pdf(data)?
    } else {
        bail!("Unsupported file type: .{ext}. Supported: text files, xlsx, docx, pptx, pdf.");
    };

    if text.len() > MAX_EXTRACTED_SIZE {
        bail!("Extracted text exceeds 500KB limit ({} KB)", text.len() / 1024);
    }

    if text.trim().is_empty() {
        bail!("File appears to be empty after extraction");
    }

    Ok(text)
}

/// Extract text from xlsx/xls using calamine
fn extract_xlsx(data: &[u8]) -> Result<String> {
    use calamine::{Reader, Xlsx, Data};
    use std::io::Cursor;

    let cursor = Cursor::new(data);
    let mut workbook: Xlsx<_> = Xlsx::new(cursor)
        .map_err(|e| anyhow::anyhow!("Failed to open spreadsheet: {e}"))?;

    let mut output = String::new();
    let sheet_names: Vec<String> = workbook.sheet_names().to_vec();

    for name in &sheet_names {
        if let Ok(range) = workbook.worksheet_range(name) {
            if sheet_names.len() > 1 {
                output.push_str(&format!("--- Sheet: {} ---\n", name));
            }
            for row in range.rows() {
                let cells: Vec<String> = row.iter().map(|cell| {
                    match cell {
                        Data::Empty => String::new(),
                        Data::String(s) => s.clone(),
                        Data::Float(f) => format!("{f}"),
                        Data::Int(i) => format!("{i}"),
                        Data::Bool(b) => format!("{b}"),
                        Data::Error(e) => format!("#{e:?}"),
                        Data::DateTime(dt) => format!("{dt}"),
                        Data::DateTimeIso(s) => s.clone(),
                        Data::DurationIso(s) => s.clone(),
                    }
                }).collect();
                output.push_str(&cells.join(","));
                output.push('\n');
            }
            output.push('\n');
        }
    }

    Ok(output)
}

/// Extract text from docx (Office Open XML — zip with word/document.xml)
fn extract_docx(data: &[u8]) -> Result<String> {
    use std::io::Cursor;

    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| anyhow::anyhow!("Failed to open docx: {e}"))?;

    let mut xml = String::new();
    if let Ok(mut file) = archive.by_name("word/document.xml") {
        use std::io::Read;
        file.read_to_string(&mut xml)
            .map_err(|e| anyhow::anyhow!("Failed to read document.xml: {e}"))?;
    } else {
        bail!("No word/document.xml found in docx");
    }

    // Simple XML text extraction: get content of <w:t> tags, newline at </w:p>
    let mut output = String::new();
    let mut in_text = false;
    let mut chars = xml.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '<' {
            let mut tag = String::new();
            for tc in chars.by_ref() {
                if tc == '>' { break; }
                tag.push(tc);
            }
            if tag.starts_with("w:t") && !tag.starts_with("w:tbl") {
                in_text = true;
            } else if tag == "/w:t" {
                in_text = false;
                output.push(' ');
            } else if tag == "/w:p" {
                output.push('\n');
            }
        } else if in_text {
            output.push(ch);
        }
    }

    Ok(output)
}

/// Extract text from pptx (Office Open XML — zip with ppt/slides/slideN.xml)
fn extract_pptx(data: &[u8]) -> Result<String> {
    use std::io::{Cursor, Read};

    let cursor = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(cursor)
        .map_err(|e| anyhow::anyhow!("Failed to open pptx: {e}"))?;

    let mut output = String::new();
    let mut slide_names: Vec<String> = Vec::new();

    for i in 0..archive.len() {
        if let Ok(file) = archive.by_index(i) {
            let name = file.name().to_string();
            if name.starts_with("ppt/slides/slide") && name.ends_with(".xml") {
                slide_names.push(name);
            }
        }
    }
    slide_names.sort();

    for (idx, name) in slide_names.iter().enumerate() {
        if let Ok(mut file) = archive.by_name(name) {
            let mut xml = String::new();
            file.read_to_string(&mut xml)?;

            output.push_str(&format!("--- Slide {} ---\n", idx + 1));

            // Extract <a:t> text elements
            let mut in_text = false;
            let mut chars = xml.chars().peekable();
            while let Some(ch) = chars.next() {
                if ch == '<' {
                    let mut tag = String::new();
                    for tc in chars.by_ref() {
                        if tc == '>' { break; }
                        tag.push(tc);
                    }
                    if tag.starts_with("a:t") && !tag.contains('/') {
                        in_text = true;
                    } else if tag == "/a:t" {
                        in_text = false;
                        output.push(' ');
                    } else if tag == "/a:p" {
                        output.push('\n');
                    }
                } else if in_text {
                    output.push(ch);
                }
            }
            output.push('\n');
        }
    }

    Ok(output)
}

/// Extract text from PDF
fn extract_pdf(data: &[u8]) -> Result<String> {
    pdf_extract::extract_text_from_mem(data)
        .map_err(|e| anyhow::anyhow!("PDF extraction failed: {e}"))
}

/// Suggest skill IDs based on file extension.
pub fn suggest_skills(filename: &str) -> Vec<String> {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "rs" => vec!["rust".into()],
        "ts" | "tsx" | "js" | "jsx" | "vue" | "svelte" => vec!["frontend".into()],
        "py" => vec!["python".into()],
        "sql" => vec!["sql".into()],
        "go" => vec!["golang".into()],
        "java" | "kt" | "kts" => vec!["java".into()],
        "swift" => vec!["swift".into()],
        "rb" => vec!["ruby".into()],
        "php" => vec!["php".into()],
        _ => vec![],
    }
}

/// A context file entry for prompt building (text or image reference).
pub struct ContextEntry {
    pub filename: String,
    pub text: String,
    pub disk_path: Option<String>,
}

/// Build the `=== CONTEXT FILES ===` prompt section.
/// Text files: inline content. Images: reference path for agent to read.
pub fn build_context_prompt(files: &[ContextEntry]) -> String {
    if files.is_empty() { return String::new(); }

    let mut parts = Vec::new();
    for entry in files {
        if let Some(ref path) = entry.disk_path {
            // Image: instruct the agent to read it with its vision/file tool
            parts.push(format!(
                "--- {} (image) ---\nIMPORTANT: This is an image file attached by the user. You MUST read and analyze it using your file reading tool.\nFile path: {}\nDo NOT describe the image without reading it first.",
                entry.filename, path
            ));
        } else {
            parts.push(format!("--- {} ---\n{}", entry.filename, entry.text));
        }
    }
    parts.join("\n\n")
}

/// Detect MIME type from extension (simple mapping for display).
pub fn mime_from_extension(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "txt" | "md" | "log" => "text/plain",
        "json" => "application/json",
        "csv" => "text/csv",
        "xml" | "html" | "htm" => "text/html",
        "yaml" | "yml" => "text/yaml",
        "pdf" => "application/pdf",
        "xlsx" | "xls" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => "text/plain",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_text_plain() {
        let text = extract_text("hello.txt", b"Hello world").unwrap();
        assert_eq!(text, "Hello world");
    }

    #[test]
    fn extract_text_csv() {
        let text = extract_text("data.csv", b"a,b,c\n1,2,3").unwrap();
        assert_eq!(text, "a,b,c\n1,2,3");
    }

    #[test]
    fn extract_text_code_file() {
        let text = extract_text("main.rs", b"fn main() {}").unwrap();
        assert_eq!(text, "fn main() {}");
    }

    #[test]
    fn extract_text_unsupported() {
        let result = extract_text("image.png", b"\x89PNG\r\n");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unsupported"));
    }

    #[test]
    fn extract_text_non_utf8() {
        let result = extract_text("bad.txt", &[0xFF, 0xFE, 0x00, 0x01]);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("UTF-8"));
    }

    #[test]
    fn extract_text_empty_file() {
        let result = extract_text("empty.txt", b"   ");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("empty"));
    }

    #[test]
    fn build_context_prompt_empty() {
        assert_eq!(build_context_prompt(&[]), "");
    }

    #[test]
    fn build_context_prompt_single() {
        let files = vec![ContextEntry { filename: "test.sql".into(), text: "SELECT 1".into(), disk_path: None }];
        let prompt = build_context_prompt(&files);
        assert!(prompt.contains("--- test.sql ---"));
        assert!(prompt.contains("SELECT 1"));
    }

    #[test]
    fn build_context_prompt_multiple() {
        let files = vec![
            ContextEntry { filename: "a.txt".into(), text: "AAA".into(), disk_path: None },
            ContextEntry { filename: "b.txt".into(), text: "BBB".into(), disk_path: None },
        ];
        let prompt = build_context_prompt(&files);
        assert!(prompt.contains("--- a.txt ---"));
        assert!(prompt.contains("--- b.txt ---"));
        assert!(prompt.contains("AAA"));
        assert!(prompt.contains("BBB"));
    }

    #[test]
    fn build_context_prompt_with_image() {
        let files = vec![
            ContextEntry { filename: "data.csv".into(), text: "a,b\n1,2".into(), disk_path: None },
            ContextEntry { filename: "screenshot.png".into(), text: "[Image: screenshot.png]".into(), disk_path: Some("/tmp/abc.png".into()) },
        ];
        let prompt = build_context_prompt(&files);
        assert!(prompt.contains("data.csv"));
        assert!(prompt.contains("a,b"));
        assert!(prompt.contains("screenshot.png"));
        assert!(prompt.contains("/tmp/abc.png"));
    }

    #[test]
    fn is_image_detection() {
        assert!(is_image("photo.png"));
        assert!(is_image("CHART.JPG"));
        assert!(is_image("icon.webp"));
        assert!(is_image("diagram.svg"));
        assert!(!is_image("data.csv"));
        assert!(!is_image("notes.txt"));
        assert!(!is_image("report.pdf"));
    }

    #[test]
    fn extract_content_image() {
        let result = extract_content("test.png", b"\x89PNG fake image data").unwrap();
        assert!(matches!(result, ExtractedContent::Image { .. }));
    }

    #[test]
    fn extract_content_text() {
        let result = extract_content("hello.txt", b"Hello").unwrap();
        assert!(matches!(result, ExtractedContent::Text(_)));
    }

    #[test]
    fn suggest_skills_rust() {
        assert_eq!(suggest_skills("main.rs"), vec!["rust"]);
    }

    #[test]
    fn suggest_skills_typescript() {
        assert!(suggest_skills("App.tsx").contains(&"frontend".into()));
    }

    #[test]
    fn suggest_skills_unknown() {
        assert!(suggest_skills("notes.txt").is_empty());
    }

    #[test]
    fn mime_detection() {
        assert_eq!(mime_from_extension("data.xlsx"), "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet");
        assert_eq!(mime_from_extension("doc.pdf"), "application/pdf");
        assert_eq!(mime_from_extension("readme.md"), "text/plain");
    }

    #[test]
    fn extensionless_files_detected() {
        // Dockerfile, Makefile etc. should be treated as text
        let text = extract_text("Makefile", b"all: build").unwrap();
        assert_eq!(text, "all: build");
    }
}
