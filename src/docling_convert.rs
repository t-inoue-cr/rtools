use std::any::Any;
use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use docling::{DocumentConverter, SourceDocument};

/// docling の `DocumentConverter` だけで文書を Markdown にする。
pub fn document_to_markdown(path: &Path) -> Result<String, String> {
    let caught = panic::catch_unwind(AssertUnwindSafe(|| markdown_from_document(path)));
    match caught {
        Ok(Ok(markdown)) => Ok(markdown),
        Ok(Err(err)) => Err(format_convert_error(path, &err)),
        Err(payload) => Err(format_convert_error(path, &panic_to_string(payload))),
    }
}

fn markdown_from_document(path: &Path) -> Result<String, String> {
    let source = SourceDocument::from_file(path).map_err(|err| err.to_string())?;
    let result = DocumentConverter::new()
        .convert(source)
        .map_err(|err| err.to_string())?;
    let markdown = result.document.export_to_markdown();
    if markdown.trim().is_empty() {
        return Err("テキストが空でした".into());
    }
    Ok(markdown)
}

fn format_convert_error(path: &Path, detail: &str) -> String {
    format!("{}: 文書の変換に失敗しました。{detail}", path.display())
}

fn panic_to_string(payload: Box<dyn Any + Send>) -> String {
    if let Some(msg) = payload.downcast_ref::<&str>() {
        (*msg).to_string()
    } else if let Some(msg) = payload.downcast_ref::<String>() {
        msg.clone()
    } else {
        "文書の変換中にパニックしました".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::PathBuf;

    fn with_temp_file(name: &str, contents: impl AsRef<[u8]>, check: impl FnOnce(&Path)) {
        let dir = std::env::temp_dir().join(format!(
            "rtools-docling-{}-{}",
            name.replace(['/', '\\', '.'], "-"),
            std::process::id()
        ));
        fs::create_dir_all(&dir).unwrap();
        let path: PathBuf = dir.join(name);
        fs::write(&path, contents).unwrap();
        check(&path);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_dir(&dir);
    }

    #[test]
    fn converts_html_to_markdown() {
        with_temp_file(
            "hello.html",
            "<!DOCTYPE html><html><body><h1>DoclingHello</h1><p>Paragraph text for conversion.</p></body></html>",
            |path| {
                let md = document_to_markdown(path).expect("document_to_markdown");
                assert!(md.contains("DoclingHello"), "unexpected markdown: {md:?}");
                assert!(
                    md.contains("Paragraph text for conversion."),
                    "unexpected markdown: {md:?}"
                );
            },
        );
    }

    #[test]
    fn converts_markdown_passthrough() {
        with_temp_file(
            "hello.md",
            "# DoclingHello\n\nParagraph text for conversion.\n",
            |path| {
                let md = document_to_markdown(path).expect("document_to_markdown");
                assert!(md.contains("DoclingHello"), "unexpected markdown: {md:?}");
                assert!(
                    md.contains("Paragraph text for conversion."),
                    "unexpected markdown: {md:?}"
                );
            },
        );
    }

    #[test]
    fn empty_html_is_an_error() {
        with_temp_file(
            "empty.html",
            "<!DOCTYPE html><html><body></body></html>",
            |path| {
                let err = document_to_markdown(path).expect_err("empty html");
                assert!(
                    err.contains("変換に失敗しました"),
                    "unexpected error: {err}"
                );
                assert!(
                    err.contains("empty.html"),
                    "error should include file name: {err}"
                );
            },
        );
    }
}
