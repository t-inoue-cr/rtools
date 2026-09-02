use std::fs;
use std::panic::{self, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

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

/// PDF から抽出したテキストと、使ったバックエンド。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedText {
    pub text: String,
    pub used_pdftotext: bool,
}

/// PDF からテキストを抽出し、ページ（フォームフィード）または文字数でチャンク化する。
pub fn structure_pdf(path: &Path) -> Result<(Vec<PdfChunk>, bool), String> {
    let extracted = extract_text(path)?;
    Ok((
        chunks_from_text(path, &extracted.text),
        extracted.used_pdftotext,
    ))
}

fn extract_text(path: &Path) -> Result<ExtractedText, String> {
    extract_text_with(path, extract_with_pdf_extract, extract_with_pdftotext)
}

/// `pdf-extract` を先に試し、失敗（Err / panic / 空テキスト）なら `pdftotext` にフォールバックする。
fn extract_text_with(
    path: &Path,
    primary: impl FnOnce(&Path) -> Result<String, String>,
    fallback: impl FnOnce(&Path) -> Result<String, String>,
) -> Result<ExtractedText, String> {
    match primary(path) {
        Ok(text) if !text.trim().is_empty() => Ok(ExtractedText {
            text,
            used_pdftotext: false,
        }),
        primary_result => {
            let primary_err = match primary_result {
                Ok(_) => "テキストが空でした".to_string(),
                Err(err) => err,
            };
            match fallback(path) {
                Ok(text) if !text.trim().is_empty() => Ok(ExtractedText {
                    text,
                    used_pdftotext: true,
                }),
                Ok(_) => Err(format_extraction_error(
                    path,
                    &primary_err,
                    "pdftotext の出力が空でした",
                )),
                Err(fallback_err) => {
                    Err(format_extraction_error(path, &primary_err, &fallback_err))
                }
            }
        }
    }
}

fn extract_with_pdf_extract(path: &Path) -> Result<String, String> {
    let path_owned = path.to_path_buf();
    let caught = panic::catch_unwind(AssertUnwindSafe(|| pdf_extract::extract_text(&path_owned)));
    match caught {
        Ok(Ok(text)) if !text.trim().is_empty() => Ok(text),
        Ok(Ok(_)) => Err("テキストが空でした".into()),
        Ok(Err(err)) => Err(err.to_string()),
        Err(payload) => Err(panic_to_string(payload)),
    }
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

fn format_extraction_error(path: &Path, primary_err: &str, fallback_err: &str) -> String {
    format!(
        "{}: 日本語特許PDF（OpenPDF / UniJIS / CMap）のテキスト抽出に失敗しました。\
         pdf-extract は UniJIS-UCS2-H などの CMap を扱えません。\
         Poppler の pdftotext を PATH に入れると抽出できます\
         （Windows では Poppler for Windows をインストールし、pdftotext.exe があるフォルダを PATH に追加してください）。\
         pdf-extract: {primary_err} / pdftotext: {fallback_err}",
        path.display()
    )
}

fn extract_with_pdftotext(path: &Path) -> Result<String, String> {
    let out_path = unique_pdftotext_out_path();
    let _guard = TempFileGuard {
        path: out_path.clone(),
    };
    match run_pdftotext(path, &out_path, true) {
        Ok(()) => read_pdftotext_output(&out_path),
        Err(err) if is_unknown_layout_option(&err) => {
            run_pdftotext(path, &out_path, false)?;
            read_pdftotext_output(&out_path)
        }
        Err(err) => Err(err),
    }
}

fn is_unknown_layout_option(err: &str) -> bool {
    let lower = err.to_ascii_lowercase();
    lower.contains("layout") && (lower.contains("unknown") || lower.contains("unrecognized"))
}

fn unique_pdftotext_out_path() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "rtools-pdftotext-{}-{nanos}.txt",
        std::process::id()
    ))
}

struct TempFileGuard {
    path: PathBuf,
}

impl Drop for TempFileGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn pdftotext_command(pdf: &Path, out: &Path, layout: bool) -> Command {
    let mut cmd = Command::new("pdftotext");
    if layout {
        cmd.arg("-layout");
    }
    cmd.arg("-enc").arg("UTF-8").arg(pdf).arg(out);
    cmd.stdin(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn run_pdftotext(pdf: &Path, out: &Path, layout: bool) -> Result<(), String> {
    let output = pdftotext_command(pdf, out, layout)
        .output()
        .map_err(|err| {
            if err.kind() == std::io::ErrorKind::NotFound {
                "pdftotext が見つかりません（PATH を確認してください）".into()
            } else {
                format!("pdftotext を起動できません: {err}")
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let detail = [stderr.trim(), stdout.trim()]
        .into_iter()
        .find(|s| !s.is_empty())
        .unwrap_or("終了コードが失敗でした");
    Err(format!("pdftotext が失敗しました: {detail}"))
}

fn read_pdftotext_output(path: &Path) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|err| format!("pdftotext の出力を読めません: {err}"))?;
    Ok(decode_pdftotext_bytes(&bytes))
}

fn decode_pdftotext_bytes(bytes: &[u8]) -> String {
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).replace("\r\n", "\n")
}

/// `pdftotext -layout` 向けのコマンドライン（テスト用）。
#[cfg(test)]
fn pdftotext_argv(pdf: &Path, out: &Path, layout: bool) -> Vec<String> {
    let mut args = Vec::new();
    if layout {
        args.push("-layout".into());
    }
    args.push("-enc".into());
    args.push("UTF-8".into());
    args.push(pdf.display().to_string());
    args.push(out.display().to_string());
    args
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
    fn uses_pdf_extract_when_it_succeeds() {
        let called = std::cell::Cell::new(false);
        let extracted = extract_text_with(
            Path::new("ok.pdf"),
            |_| Ok("本文です".into()),
            |_| {
                called.set(true);
                Ok("fallback".into())
            },
        )
        .unwrap();
        assert_eq!(extracted.text, "本文です");
        assert!(!extracted.used_pdftotext);
        assert!(!called.get());
    }

    #[test]
    fn falls_back_when_pdf_extract_errors() {
        let extracted = extract_text_with(
            Path::new("patent.pdf"),
            |_| Err("unsupported encoding UniJIS-UCS2-H".into()),
            |_| Ok("要約\n特許請求の範囲".into()),
        )
        .unwrap();
        assert!(extracted.used_pdftotext);
        assert!(extracted.text.contains("特許請求の範囲"));
    }

    #[test]
    fn falls_back_when_pdf_extract_returns_empty() {
        let extracted = extract_text_with(
            Path::new("empty.pdf"),
            |_| Ok("   \n".into()),
            |_| Ok("フォールバック本文".into()),
        )
        .unwrap();
        assert!(extracted.used_pdftotext);
        assert_eq!(extracted.text, "フォールバック本文");
    }

    #[test]
    fn both_backends_fail_mentions_unijis_and_poppler() {
        let err = extract_text_with(
            Path::new("C:\\patents\\jp.pdf"),
            |_| Err("unsupported encoding UniJIS-UCS2-H".into()),
            |_| Err("pdftotext が見つかりません（PATH を確認してください）".into()),
        )
        .unwrap_err();
        assert!(err.contains("UniJIS"));
        assert!(err.contains("CMap"));
        assert!(err.contains("pdftotext"));
        assert!(err.contains("Poppler"));
        assert!(err.contains("UniJIS-UCS2-H"));
        assert!(err.contains("jp.pdf"));
    }

    #[test]
    fn pdftotext_argv_prefers_layout_and_utf8_file() {
        let args = pdftotext_argv(Path::new("in.pdf"), Path::new("out.txt"), true);
        assert_eq!(args, vec!["-layout", "-enc", "UTF-8", "in.pdf", "out.txt"]);
    }

    #[test]
    fn decode_pdftotext_strips_utf8_bom_and_crlf() {
        let mut bytes = vec![0xEF, 0xBB, 0xBF];
        bytes.extend_from_slice("要約\r\n特許請求の範囲".as_bytes());
        assert_eq!(decode_pdftotext_bytes(&bytes), "要約\n特許請求の範囲");
    }

    #[test]
    fn panic_payload_keeps_unijis_message() {
        let msg = panic_to_string(Box::new("unsupported encoding UniJIS-UCS2-H"));
        assert!(msg.contains("UniJIS-UCS2-H"));
        let msg = panic_to_string(Box::new("owned".to_string()));
        assert_eq!(msg, "owned");
    }

    #[test]
    fn pdftotext_writes_and_reads_temp_file_if_installed() {
        if Command::new("pdftotext").arg("-v").output().is_err() {
            return;
        }
        let dir = std::env::temp_dir().join(format!(
            "rtools-pdftotext-it-{}",
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let pdf = dir.join("hello.pdf");
        fs::write(&pdf, include_bytes!("../tests/fixtures/hello.pdf")).unwrap();
        let text = extract_with_pdftotext(&pdf).expect("pdftotext fallback");
        assert!(
            text.contains("HelloPopplerFallback"),
            "unexpected pdftotext output: {text:?}"
        );
        let _ = fs::remove_file(&pdf);
        let _ = fs::remove_dir(&dir);
    }
}
