use std::fs;
use std::path::PathBuf;

use rtools::pdf::{collect_pdfs, markdown_file_name, pdf_to_markdown};

/// 折り返しで空行が入っていた箇所が、1 つの本文として結合されていること。
fn expected_joined_phrases(stem: &str) -> &'static [&'static str] {
    if stem.contains("2026081297") {
        &["特性排気速度効率を向上できる"]
    } else if stem.contains("2026082616") {
        &["第１の端部、第２の端部、チャンバを囲む"]
    } else if stem.contains("2026086507") {
        &["衛星分離後に軌道降下したロケットを、安全に回収し"]
    } else if stem.contains("2026129840") {
        &["２００μｍより大きい体積平均サイズ"]
    } else {
        &[]
    }
}

#[test]
fn converts_sample_pdfs_to_markdown() {
    let sample = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("sample");
    if !sample.is_dir() {
        eprintln!(
            "skipping: sample directory not found at {}",
            sample.display()
        );
        return;
    }

    let pdfs = collect_pdfs(&sample).unwrap_or_else(|err| panic!("collect_pdfs: {err}"));
    if pdfs.is_empty() {
        eprintln!("skipping: no PDFs in {}", sample.display());
        return;
    }

    for pdf in pdfs {
        let markdown =
            pdf_to_markdown(&pdf).unwrap_or_else(|err| panic!("{}: {err}", pdf.display()));
        let stem = pdf
            .file_stem()
            .map(|name| name.to_string_lossy())
            .expect("pdf stem");
        let title = format!("# {stem}\n\n");
        assert!(
            markdown.starts_with(&title),
            "{}: unexpected title: {markdown:?}",
            pdf.display()
        );
        let body = markdown[title.len()..].trim();
        assert!(
            !body.is_empty(),
            "{}: markdown body is empty",
            pdf.display()
        );
        assert!(
            !markdown.contains("## ページ"),
            "{}: unexpected page heading: {markdown:?}",
            pdf.display()
        );
        for phrase in expected_joined_phrases(&stem) {
            assert!(
                markdown.contains(phrase),
                "{}: wrapped phrase not joined: {phrase}\n{markdown}",
                pdf.display()
            );
        }

        let md_path = pdf.with_file_name(markdown_file_name(&pdf));
        fs::write(&md_path, markdown.as_bytes())
            .unwrap_or_else(|err| panic!("{}: {err}", md_path.display()));
    }
}
