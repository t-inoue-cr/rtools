use std::fs;
use std::path::PathBuf;

use rtools::pdf::{collect_pdfs, markdown_file_name, pdf_to_markdown};

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

        let md_path = pdf.with_file_name(markdown_file_name(&pdf));
        fs::write(&md_path, markdown.as_bytes())
            .unwrap_or_else(|err| panic!("{}: {err}", md_path.display()));
    }
}
