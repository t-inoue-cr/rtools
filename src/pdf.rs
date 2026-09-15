use std::fmt::Write;
use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};

use pdf_oxide::api::Pdf;
use pdf_oxide::layout::TextLine;
use pdf_oxide::structure::Table;

use crate::pdf_layout::{
    LayoutLine, LayoutTable, PageBlock, blocks_to_markdown, blocks_to_plain, join_fragments,
    reconstruct_page,
};

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

/// PDF から Markdown を生成する（ページ見出しは付けない）。
pub fn pdf_to_markdown(path: &Path) -> Result<String, String> {
    let path_owned = path.to_path_buf();
    let caught = panic::catch_unwind(AssertUnwindSafe(|| markdown_from_pdf(&path_owned)));
    match caught {
        Ok(Ok(markdown)) => Ok(markdown),
        Ok(Err(err)) => Err(format_extraction_error(path, &err)),
        Err(payload) => Err(format_extraction_error(path, &panic_to_string(payload))),
    }
}

/// 保存ダイアログ用の初期ファイル名（`{stem}.md`）。
pub fn markdown_file_name(path: &Path) -> String {
    match path.file_stem() {
        Some(stem) if !stem.is_empty() => format!("{}.md", stem.to_string_lossy()),
        _ => "converted.md".into(),
    }
}

fn markdown_from_pdf(path: &Path) -> Result<String, String> {
    let mut out = String::new();
    push_markdown_title(&mut out, path);
    let mut wrote_page = false;
    for_each_page_blocks(path, |_, blocks| {
        push_markdown_page(&mut out, &mut wrote_page, &blocks_to_markdown(&blocks));
        Ok(())
    })?;
    if !wrote_page {
        return Err("テキストが空でした".into());
    }
    Ok(out)
}

#[cfg(test)]
fn markdown_from_text(path: &Path, text: &str) -> String {
    let mut out = String::with_capacity(text.len().saturating_add(64));
    push_markdown_title(&mut out, path);
    let mut wrote_page = false;
    for page in text.split('\u{c}') {
        push_markdown_page(&mut out, &mut wrote_page, &collapse_ws(page));
    }
    out
}

fn push_markdown_title(out: &mut String, path: &Path) {
    let title = path
        .file_stem()
        .map(|name| name.to_string_lossy())
        .unwrap_or_else(|| path.display().to_string().into());
    let _ = write!(out, "# {title}\n\n");
}

fn push_markdown_page(out: &mut String, wrote_page: &mut bool, body: &str) {
    if body.trim().is_empty() {
        return;
    }
    if *wrote_page {
        out.push('\n');
    }
    out.push_str(body);
    if !body.ends_with('\n') {
        out.push('\n');
    }
    *wrote_page = true;
}

fn extract_text(path: &Path) -> Result<String, String> {
    let path_owned = path.to_path_buf();
    let caught = panic::catch_unwind(AssertUnwindSafe(|| extract_with_pdf_oxide(&path_owned)));
    match caught {
        Ok(Ok(text)) if !text.trim().is_empty() => Ok(text),
        Ok(Ok(_)) => Err(format_extraction_error(path, "テキストが空でした")),
        Ok(Err(err)) => Err(format_extraction_error(path, &err)),
        Err(payload) => Err(format_extraction_error(path, &panic_to_string(payload))),
    }
}

fn extract_with_pdf_oxide(path: &Path) -> Result<String, String> {
    let mut pages = Vec::new();
    for_each_page_blocks(path, |_, blocks| {
        pages.push(blocks_to_plain(&blocks));
        Ok(())
    })?;
    Ok(pages.join("\u{c}"))
}

fn for_each_page_blocks(
    path: &Path,
    mut on_page: impl FnMut(u32, Vec<PageBlock>) -> Result<(), String>,
) -> Result<(), String> {
    let mut doc = Pdf::open(path).map_err(|err| format!("PDF を開けません: {err}"))?;
    let page_count = doc
        .page_count()
        .map_err(|err| format!("ページ数を取得できません: {err}"))?;
    if page_count == 0 {
        return Err("ページがありません".into());
    }

    for i in 0..page_count {
        if let Some(blocks) = extract_page_blocks(&mut doc, i)? {
            on_page((i + 1) as u32, blocks)?;
        }
    }
    Ok(())
}

fn extract_page_blocks(doc: &mut Pdf, index: usize) -> Result<Option<Vec<PageBlock>>, String> {
    let lines = match doc.extract_text_lines(index) {
        Ok(lines) if !lines.is_empty() => lines,
        Ok(_) | Err(_) => return fallback_page_blocks(doc, index),
    };
    let tables = doc.extract_tables(index).unwrap_or_default();
    let layout_lines: Vec<LayoutLine> = lines.into_iter().map(layout_line_from_pdf).collect();
    let layout_tables: Vec<LayoutTable> = tables.iter().filter_map(layout_table_from_pdf).collect();
    let blocks = reconstruct_page(layout_lines, layout_tables);
    if blocks.is_empty() {
        return fallback_page_blocks(doc, index);
    }
    Ok(Some(blocks))
}

fn fallback_page_blocks(doc: &mut Pdf, index: usize) -> Result<Option<Vec<PageBlock>>, String> {
    let page_no = index + 1;
    let text = doc
        .to_text(index)
        .map_err(|err| format!("ページ {page_no} の抽出に失敗しました: {err}"))?;
    let body = collapse_ws(&text);
    if body.is_empty() {
        Ok(None)
    } else {
        Ok(Some(vec![PageBlock::Paragraph(body)]))
    }
}

fn layout_line_from_pdf(line: TextLine) -> LayoutLine {
    let font_size = if line.words.is_empty() {
        line.bbox.height.max(1.0)
    } else {
        let sum: f32 = line.words.iter().map(|word| word.avg_font_size).sum();
        (sum / line.words.len() as f32).max(1.0)
    };
    let text = if line.words.is_empty() {
        line.text
    } else {
        join_fragments(line.words.iter().map(|word| word.text.as_str()))
    };
    LayoutLine {
        text,
        x: line.bbox.x,
        y: line.bbox.y,
        width: line.bbox.width,
        height: line.bbox.height,
        font_size,
        heading_level: None,
    }
}

fn layout_table_from_pdf(table: &Table) -> Option<LayoutTable> {
    if !table.is_real_grid() {
        return None;
    }
    let bbox = table.bbox?;
    let col_count = table.col_count.max(
        table
            .rows
            .iter()
            .map(|row| {
                row.cells
                    .iter()
                    .map(|cell| cell.colspan.max(1) as usize)
                    .sum::<usize>()
            })
            .max()
            .unwrap_or(0),
    );
    let mut rows = Vec::new();
    for row in &table.rows {
        let mut cells = Vec::new();
        for cell in &row.cells {
            cells.push(cell.text.clone());
            for _ in 1..cell.colspan.max(1) {
                cells.push(String::new());
            }
        }
        while cells.len() < col_count {
            cells.push(String::new());
        }
        if !cells.is_empty() {
            rows.push(cells);
        }
    }
    let has_header =
        table.has_header || table.rows.first().map(|row| row.is_header).unwrap_or(false);
    Some(LayoutTable {
        x: bbox.x,
        y: bbox.y,
        width: bbox.width,
        height: bbox.height,
        rows,
        has_header,
    })
}

fn format_extraction_error(path: &Path, detail: &str) -> String {
    format!(
        "{}: PDF のテキスト抽出に失敗しました。{detail}",
        path.display()
    )
}

fn panic_to_string(payload: Box<dyn std::any::Any + Send>) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        (*msg).to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        "PDF 解析中にパニックしました".into()
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

    #[test]
    fn extraction_error_is_japanese() {
        let err = format_extraction_error(Path::new("C:\\patents\\jp.pdf"), "ページがありません");
        assert!(err.contains("テキスト抽出に失敗しました"));
        assert!(err.contains("jp.pdf"));
        assert!(err.contains("ページがありません"));
    }

    #[test]
    fn panic_payload_keeps_message() {
        let msg = panic_to_string(Box::new("parse failed"));
        assert_eq!(msg, "parse failed");
        let msg = panic_to_string(Box::new("owned".to_string()));
        assert_eq!(msg, "owned");
    }

    #[test]
    fn markdown_file_name_uses_stem() {
        assert_eq!(
            markdown_file_name(Path::new("/docs/report.pdf")),
            "report.md"
        );
        assert_eq!(markdown_file_name(Path::new("a.PDF")), "a.md");
        assert_eq!(markdown_file_name(Path::new("noext")), "noext.md");
    }

    #[test]
    fn markdown_uses_stem_as_title() {
        let md = markdown_from_text(Path::new("/docs/report.pdf"), "hello");
        assert_eq!(md, "# report\n\nhello\n");
        assert!(!md.contains("## ページ"));
    }

    #[test]
    fn markdown_splits_form_feed_as_pages() {
        let md = markdown_from_text(Path::new("a.pdf"), "page-one\u{c}page-two");
        assert_eq!(md, "# a\n\npage-one\n\npage-two\n");
        assert!(!md.contains("## ページ"));
    }

    #[test]
    fn markdown_skips_empty_pages() {
        let md = markdown_from_text(Path::new("a.pdf"), "keep\u{c}  \n\t\u{c}also");
        assert_eq!(md, "# a\n\nkeep\n\nalso\n");
        assert!(!md.contains("## ページ"));
    }

    #[test]
    fn pdf_to_markdown_includes_fixture_text() {
        let dir = std::env::temp_dir().join(format!("rtools-pdf-md-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("hello.pdf");
        fs::write(&pdf, include_bytes!("../tests/fixtures/hello.pdf")).unwrap();
        let md = pdf_to_markdown(&pdf).expect("pdf_to_markdown");
        assert!(md.starts_with("# hello\n\n"), "unexpected markdown: {md:?}");
        assert!(md.contains("HelloPdfOxide"), "unexpected markdown: {md:?}");
        assert!(!md.contains("## ページ"), "unexpected markdown: {md:?}");
        let _ = fs::remove_file(&pdf);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn pdf_oxide_extracts_fixture_pages() {
        let dir = std::env::temp_dir().join(format!("rtools-pdf-oxide-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("hello.pdf");
        fs::write(&pdf, include_bytes!("../tests/fixtures/hello.pdf")).unwrap();
        let text = extract_text(&pdf).expect("pdf_oxide extraction");
        assert!(
            text.contains("HelloPdfOxide"),
            "unexpected extracted text: {text:?}"
        );
        let chunks = structure_pdf(&pdf).expect("structure");
        assert!(!chunks.is_empty());
        assert!(chunks.iter().any(|c| c.text.contains("HelloPdfOxide")));
        let _ = fs::remove_file(&pdf);
        let _ = fs::remove_dir(&dir);
    }
}
