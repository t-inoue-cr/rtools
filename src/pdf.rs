use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

/// OpenSearch に登録する 1 チャンク（ページ内のテキスト断片）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PdfChunk {
    pub path: String,
    pub file_name: String,
    pub title: String,
    pub page: u32,
    pub chunk: u32,
    pub text: String,
}

const TARGET_CHARS: usize = 1800;
const OVERLAP_CHARS: usize = 120;

/// フォルダ以下の PDF を再帰的に集める（隠しディレクトリはスキップ）。
pub fn collect_pdfs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut out = Vec::new();
    walk_pdfs(root, &mut out, 0)?;
    out.sort();
    Ok(out)
}

fn walk_pdfs(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) -> Result<(), String> {
    if depth > 12 {
        return Ok(());
    }
    if out.len() >= 2000 {
        return Ok(());
    }
    let entries = fs::read_dir(dir).map_err(|err| format!("{}: {err}", dir.display()))?;
    for entry in entries {
        let entry = entry.map_err(|err| format!("{}: {err}", dir.display()))?;
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|err| format!("{}: {err}", path.display()))?;
        if file_type.is_dir() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with('.') || name == "target" {
                continue;
            }
            walk_pdfs(&path, out, depth + 1)?;
        } else if file_type.is_file() && is_pdf(&path) {
            out.push(path);
            if out.len() >= 2000 {
                return Ok(());
            }
        }
    }
    Ok(())
}

pub fn is_pdf(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.eq_ignore_ascii_case("pdf"))
        .unwrap_or(false)
}

/// PDF からテキストを抽出し、ページ（フォームフィード）または文字数でチャンク化する。
pub fn structure_pdf(path: &Path) -> Result<Vec<PdfChunk>, String> {
    let text = extract_text(path)?;
    Ok(chunks_from_text(path, &text))
}

fn extract_text(path: &Path) -> Result<String, String> {
    let path_owned = path.to_path_buf();
    match panic::catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text(&path_owned))) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(err)) => Err(format!("{}: {err}", path.display())),
        Err(_) => Err(format!("{}: PDF 解析中に失敗しました", path.display())),
    }
}

pub fn chunks_from_text(path: &Path, text: &str) -> Vec<PdfChunk> {
    let path_str = path.display().to_string();
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path_str.clone());
    let title = path
        .file_stem()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| file_name.clone());

    let pages: Vec<&str> = if text.contains('\u{c}') {
        text.split('\u{c}').collect()
    } else {
        vec![text]
    };

    let mut docs = Vec::new();
    for (page_i, page_text) in pages.iter().enumerate() {
        let page = (page_i + 1) as u32;
        let pieces = split_chunks(page_text);
        if pieces.is_empty() {
            continue;
        }
        for (chunk_i, piece) in pieces.into_iter().enumerate() {
            docs.push(PdfChunk {
                path: path_str.clone(),
                file_name: file_name.clone(),
                title: title.clone(),
                page,
                chunk: (chunk_i + 1) as u32,
                text: piece,
            });
        }
    }

    if docs.is_empty() {
        docs.push(PdfChunk {
            path: path_str,
            file_name,
            title,
            page: 1,
            chunk: 1,
            text: "(テキストを抽出できませんでした)".into(),
        });
    }
    docs
}

fn split_chunks(text: &str) -> Vec<String> {
    let normalized = collapse_ws(text);
    if normalized.is_empty() {
        return Vec::new();
    }
    if normalized.chars().count() <= TARGET_CHARS {
        return vec![normalized];
    }

    let chars: Vec<char> = normalized.chars().collect();
    let mut out = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let mut end = (start + TARGET_CHARS).min(chars.len());
        if end < chars.len() {
            let window_start = start + TARGET_CHARS / 2;
            if let Some(rel) = chars[window_start..end]
                .iter()
                .rposition(|c| *c == '。' || *c == '\n' || *c == '.')
            {
                end = window_start + rel + 1;
            }
        }
        let piece: String = chars[start..end].iter().collect();
        let piece = piece.trim().to_string();
        if !piece.is_empty() {
            out.push(piece);
        }
        if end >= chars.len() {
            break;
        }
        start = end.saturating_sub(OVERLAP_CHARS);
        if start >= end {
            start = end;
        }
    }
    out
}

fn collapse_ws(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev_space = true;
    for ch in text.chars() {
        if ch == '\u{c}' {
            continue;
        }
        if ch.is_whitespace() {
            if !prev_space && !out.is_empty() {
                out.push(if ch == '\n' { '\n' } else { ' ' });
                prev_space = true;
            }
        } else {
            out.push(ch);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_short_text_as_one() {
        let path = Path::new("/docs/report.pdf");
        let docs = chunks_from_text(path, "これはテストです。");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].title, "report");
        assert_eq!(docs[0].file_name, "report.pdf");
        assert_eq!(docs[0].page, 1);
        assert_eq!(docs[0].chunk, 1);
        assert!(docs[0].text.contains("テスト"));
    }

    #[test]
    fn splits_form_feed_as_pages() {
        let path = Path::new("a.pdf");
        let docs = chunks_from_text(path, "page-one\u{c}page-two");
        assert_eq!(docs.len(), 2);
        assert_eq!(docs[0].page, 1);
        assert_eq!(docs[0].text, "page-one");
        assert_eq!(docs[1].page, 2);
        assert_eq!(docs[1].text, "page-two");
    }

    #[test]
    fn empty_text_gets_placeholder() {
        let docs = chunks_from_text(Path::new("empty.pdf"), "   \n\t");
        assert_eq!(docs.len(), 1);
        assert!(docs[0].text.contains("抽出できませんでした"));
    }

    #[test]
    fn splits_long_text() {
        let long = "あ".repeat(TARGET_CHARS + 200);
        let docs = chunks_from_text(Path::new("long.pdf"), &long);
        assert!(docs.len() >= 2);
        assert_eq!(docs[0].chunk, 1);
        assert_eq!(docs[1].chunk, 2);
    }

    #[test]
    fn detects_pdf_extension() {
        assert!(is_pdf(Path::new("a.PDF")));
        assert!(!is_pdf(Path::new("a.txt")));
    }
}
