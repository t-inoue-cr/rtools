use std::sync::mpsc::Sender;
use std::time::Duration;

use chrono::Utc;
use reqwest::blocking::{Client, RequestBuilder};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE, HeaderMap, HeaderValue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::pdf::{PdfChunk, structure_pdf};
use crate::settings::OpenSearchSettings;

const BULK_BATCH: usize = 80;
const SEARCH_SIZE: usize = 50;

#[derive(Debug, Clone)]
pub struct IngestReport {
    pub files_ok: usize,
    pub files_fail: usize,
    pub chunks: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub score: f64,
    pub title: String,
    pub path: String,
    pub page: u32,
    pub snippet: String,
}

#[derive(Debug, Clone)]
pub struct SearchOutcome {
    pub total: u64,
    pub hits: Vec<SearchHit>,
}

pub fn ingest_files(
    settings: &OpenSearchSettings,
    paths: &[std::path::PathBuf],
    log: &Sender<String>,
) -> Result<IngestReport, String> {
    let mut files_ok = 0usize;
    let mut files_fail = 0usize;
    let mut errors = Vec::new();
    let mut chunks = Vec::new();

    for (i, path) in paths.iter().enumerate() {
        let _ = log.send(format!(
            "PDF を解析しています ({}/{}): {}",
            i + 1,
            paths.len(),
            path.display()
        ));
        match structure_pdf(path) {
            Ok(docs) => {
                files_ok += 1;
                chunks.extend(docs);
            }
            Err(err) => {
                files_fail += 1;
                errors.push(err);
            }
        }
    }

    if chunks.is_empty() {
        return Ok(IngestReport {
            files_ok,
            files_fail,
            chunks: 0,
            errors,
        });
    }

    let _ = log.send(format!(
        "{} 件の PDF から {} チャンクを OpenSearch に登録します。",
        files_ok,
        chunks.len()
    ));

    let mut report = ingest_pdfs(settings, &chunks, log)?;
    report.files_ok = files_ok;
    report.files_fail = files_fail;
    report.errors.splice(0..0, errors);
    Ok(report)
}

pub fn ingest_pdfs(
    settings: &OpenSearchSettings,
    chunks: &[PdfChunk],
    log: &Sender<String>,
) -> Result<IngestReport, String> {
    if chunks.is_empty() {
        return Ok(IngestReport {
            files_ok: 0,
            files_fail: 0,
            chunks: 0,
            errors: Vec::new(),
        });
    }

    let client = build_client(settings)?;
    ensure_index(&client, settings)?;

    let ingested_at = Utc::now().to_rfc3339();
    let mut indexed = 0usize;
    let mut errors = Vec::new();

    for batch in chunks.chunks(BULK_BATCH) {
        match bulk_index(&client, settings, batch, &ingested_at) {
            Ok(n) => indexed += n,
            Err(err) => errors.push(err),
        }
        let _ = log.send(format!("登録進捗: {indexed}/{} チャンク", chunks.len()));
    }

    if indexed == 0 && !errors.is_empty() {
        return Err(errors.join(" / "));
    }

    Ok(IngestReport {
        files_ok: 0,
        files_fail: 0,
        chunks: indexed,
        errors,
    })
}

pub fn search(settings: &OpenSearchSettings, query: &str) -> Result<SearchOutcome, String> {
    let query = query.trim();
    if query.is_empty() {
        return Err("検索キーワードが空です。".into());
    }

    let client = build_client(settings)?;
    let url = format!("{}/{}/_search", settings.base_url(), settings.index());
    let body = json!({
        "size": SEARCH_SIZE,
        "track_total_hits": true,
        "query": {
            "multi_match": {
                "query": query,
                "fields": ["text^3", "title^2", "file_name"],
                "type": "best_fields",
                "operator": "and"
            }
        },
        "highlight": {
            "pre_tags": ["【"],
            "post_tags": ["】"],
            "fields": {
                "text": {
                    "fragment_size": 160,
                    "number_of_fragments": 2
                }
            }
        }
    });

    let response = with_auth(client.post(&url), settings)
        .header(CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .map_err(|err| format!("OpenSearch 検索に失敗しました: {err}"))?;

    let status = response.status();
    let text = response
        .text()
        .map_err(|err| format!("検索レスポンスを読めません: {err}"))?;
    if !status.is_success() {
        return Err(format!(
            "検索が拒否されました ({status}): {}",
            truncate(&text, 400)
        ));
    }

    parse_search_response(&text)
}

pub fn parse_search_response(text: &str) -> Result<SearchOutcome, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|err| format!("検索結果の JSON 解析に失敗しました: {err}"))?;
    let hits_obj = value
        .get("hits")
        .ok_or_else(|| "検索結果に hits がありません。".to_string())?;
    let total = parse_total(hits_obj.get("total"));
    let mut hits = Vec::new();
    if let Some(arr) = hits_obj.get("hits").and_then(Value::as_array) {
        for hit in arr {
            hits.push(parse_hit(hit));
        }
    }
    Ok(SearchOutcome { total, hits })
}

fn parse_total(total: Option<&Value>) -> u64 {
    match total {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::Object(obj)) => obj.get("value").and_then(Value::as_u64).unwrap_or(0),
        _ => 0,
    }
}

fn parse_hit(hit: &Value) -> SearchHit {
    let score = hit.get("_score").and_then(Value::as_f64).unwrap_or(0.0);
    let source = hit.get("_source").cloned().unwrap_or(Value::Null);
    let title = string_field(&source, "title")
        .or_else(|| string_field(&source, "file_name"))
        .unwrap_or_else(|| "(無題)".into());
    let path = string_field(&source, "path").unwrap_or_default();
    let page = source.get("page").and_then(Value::as_u64).unwrap_or(1) as u32;
    let snippet = highlight_snippet(hit)
        .or_else(|| string_field(&source, "text").map(|text| truncate(&text, 220)))
        .unwrap_or_default();
    SearchHit {
        score,
        title,
        path,
        page,
        snippet,
    }
}

fn highlight_snippet(hit: &Value) -> Option<String> {
    let fragments = hit.get("highlight")?.get("text")?.as_array()?;
    let joined = fragments
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join(" … ");
    if joined.is_empty() {
        None
    } else {
        Some(joined)
    }
}

fn string_field(value: &Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

fn build_client(settings: &OpenSearchSettings) -> Result<Client, String> {
    let mut builder = Client::builder()
        .timeout(Duration::from_secs(120))
        .connect_timeout(Duration::from_secs(15))
        .user_agent("rtools/0.1");
    if settings.insecure_skip_tls_verify {
        builder = builder
            .danger_accept_invalid_certs(true)
            .danger_accept_invalid_hostnames(true);
    }
    builder
        .build()
        .map_err(|err| format!("HTTP クライアントを作成できません: {err}"))
}

fn with_auth(req: RequestBuilder, settings: &OpenSearchSettings) -> RequestBuilder {
    let api_key = settings.api_key.trim();
    if !api_key.is_empty() {
        return req.header(AUTHORIZATION, format!("ApiKey {api_key}"));
    }
    let username = settings.username.trim();
    if !username.is_empty() {
        return req.basic_auth(username, Some(settings.password.as_str()));
    }
    req
}

fn ensure_index(client: &Client, settings: &OpenSearchSettings) -> Result<(), String> {
    let url = format!("{}/{}", settings.base_url(), settings.index());
    let head = with_auth(client.head(&url), settings)
        .send()
        .map_err(|err| format!("インデックス確認に失敗しました: {err}"))?;
    if head.status().is_success() {
        return Ok(());
    }

    let mapping = json!({
        "settings": {
            "number_of_shards": 1,
            "number_of_replicas": 0
        },
        "mappings": {
            "properties": {
                "title": { "type": "text", "fields": { "keyword": { "type": "keyword", "ignore_above": 256 } } },
                "file_name": { "type": "keyword" },
                "path": { "type": "keyword" },
                "page": { "type": "integer" },
                "chunk": { "type": "integer" },
                "text": { "type": "text" },
                "ingested_at": { "type": "date" }
            }
        }
    });

    let put = with_auth(client.put(&url), settings)
        .header(CONTENT_TYPE, "application/json")
        .json(&mapping)
        .send()
        .map_err(|err| format!("インデックス作成に失敗しました: {err}"))?;
    let status = put.status();
    let body = put.text().unwrap_or_default();
    if status.is_success() || body.contains("resource_already_exists_exception") {
        return Ok(());
    }
    Err(format!(
        "インデックス '{}' を作成できません ({status}): {}",
        settings.index(),
        truncate(&body, 400)
    ))
}

fn bulk_index(
    client: &Client,
    settings: &OpenSearchSettings,
    batch: &[PdfChunk],
    ingested_at: &str,
) -> Result<usize, String> {
    let body = build_bulk_body(settings.index(), batch, ingested_at);
    let url = format!("{}/_bulk", settings.base_url());
    let mut headers = HeaderMap::new();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );

    let response = with_auth(client.post(&url), settings)
        .headers(headers)
        .body(body)
        .send()
        .map_err(|err| format!("一括登録に失敗しました: {err}"))?;
    let status = response.status();
    let text = response
        .text()
        .map_err(|err| format!("一括登録の応答を読めません: {err}"))?;
    if !status.is_success() {
        return Err(format!(
            "一括登録が拒否されました ({status}): {}",
            truncate(&text, 400)
        ));
    }
    parse_bulk_indexed(&text, batch.len())
}

pub fn build_bulk_body(index: &str, batch: &[PdfChunk], ingested_at: &str) -> String {
    let mut body = String::new();
    for chunk in batch {
        let id = document_id(chunk);
        let action = json!({
            "index": {
                "_index": index,
                "_id": id
            }
        });
        let source = json!({
            "title": chunk.title,
            "file_name": chunk.file_name,
            "path": chunk.path,
            "page": chunk.page,
            "chunk": chunk.chunk,
            "text": chunk.text,
            "ingested_at": ingested_at,
        });
        body.push_str(&action.to_string());
        body.push('\n');
        body.push_str(&source.to_string());
        body.push('\n');
    }
    body
}

pub fn document_id(chunk: &PdfChunk) -> String {
    let mut hasher = Sha256::new();
    hasher.update(chunk.path.as_bytes());
    hasher.update([0]);
    hasher.update(chunk.page.to_le_bytes());
    hasher.update(chunk.chunk.to_le_bytes());
    format!("{:x}", hasher.finalize())
}

fn parse_bulk_indexed(text: &str, fallback: usize) -> Result<usize, String> {
    let value: Value = serde_json::from_str(text)
        .map_err(|err| format!("一括登録の JSON 解析に失敗しました: {err}"))?;
    if value
        .get("errors")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let mut first_err = "一括登録で一部エラーが発生しました".to_string();
        if let Some(items) = value.get("items").and_then(Value::as_array) {
            for item in items {
                if let Some(index) = item.get("index")
                    && let Some(err) = index.get("error")
                {
                    first_err = format!(
                        "一括登録エラー: {}",
                        err.get("reason")
                            .and_then(Value::as_str)
                            .unwrap_or(&err.to_string())
                    );
                    break;
                }
            }
            let ok = items
                .iter()
                .filter(|item| {
                    item.get("index")
                        .and_then(|idx| idx.get("status"))
                        .and_then(Value::as_u64)
                        .map(|status| (200..300).contains(&status))
                        .unwrap_or(false)
                })
                .count();
            if ok == 0 {
                return Err(first_err);
            }
            return Ok(ok);
        }
        return Err(first_err);
    }
    let took_items = value
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(fallback);
    Ok(took_items)
}

fn truncate(text: &str, max_chars: usize) -> String {
    let mut chars = text.chars();
    let taken: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdf::PdfChunk;
    use std::path::Path;

    fn sample_chunk() -> PdfChunk {
        PdfChunk {
            path: "/tmp/a.pdf".into(),
            file_name: "a.pdf".into(),
            title: "a".into(),
            page: 1,
            chunk: 1,
            text: "hello world".into(),
        }
    }

    #[test]
    fn bulk_body_is_ndjson() {
        let body = build_bulk_body("pdf_docs", &[sample_chunk()], "2026-01-01T00:00:00Z");
        let lines: Vec<_> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("\"index\""));
        assert!(lines[1].contains("hello world"));
        assert!(body.ends_with('\n'));
    }

    #[test]
    fn document_ids_are_stable() {
        let a = document_id(&sample_chunk());
        let mut other = sample_chunk();
        other.page = 2;
        let b = document_id(&other);
        assert_ne!(a, b);
        assert_eq!(a, document_id(&sample_chunk()));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn parses_search_hits() {
        let json = r#"{
            "hits": {
                "total": { "value": 2, "relation": "eq" },
                "hits": [
                    {
                        "_score": 1.5,
                        "_source": {
                            "title": "仕様書",
                            "path": "C:\\docs\\a.pdf",
                            "page": 3,
                            "text": "長い本文"
                        },
                        "highlight": { "text": ["【キーワード】を含む"] }
                    }
                ]
            }
        }"#;
        let out = parse_search_response(json).unwrap();
        assert_eq!(out.total, 2);
        assert_eq!(out.hits.len(), 1);
        assert_eq!(out.hits[0].title, "仕様書");
        assert_eq!(out.hits[0].page, 3);
        assert!(out.hits[0].snippet.contains("キーワード"));
    }

    #[test]
    fn parse_total_as_number() {
        let json = r#"{ "hits": { "total": 4, "hits": [] } }"#;
        let out = parse_search_response(json).unwrap();
        assert_eq!(out.total, 4);
        assert!(out.hits.is_empty());
    }

    #[test]
    fn chunks_from_text_used_in_id_path() {
        let chunks = crate::pdf::chunks_from_text(Path::new("n.pdf"), "abc");
        assert_eq!(chunks.len(), 1);
        assert!(!document_id(&chunks[0]).is_empty());
    }

    #[test]
    fn ingest_and_search_against_mock_opensearch() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            while let Ok((mut stream, _)) = listener.accept() {
                let _ = handle_mock_conn(&mut stream);
            }
        });

        let settings = OpenSearchSettings {
            url: format!("http://127.0.0.1:{port}"),
            host: "127.0.0.1".into(),
            port,
            scheme: "http".into(),
            index: "pdf_docs".into(),
            username: String::new(),
            password: String::new(),
            api_key: String::new(),
            insecure_skip_tls_verify: false,
        };
        let (log_tx, _log_rx) = std::sync::mpsc::channel();
        let report = ingest_pdfs(&settings, &[sample_chunk()], &log_tx).unwrap();
        assert_eq!(report.chunks, 1);

        let outcome = search(&settings, "hello").unwrap();
        assert_eq!(outcome.total, 1);
        assert_eq!(outcome.hits[0].title, "a");
        assert!(outcome.hits[0].snippet.contains("hello"));
    }

    fn handle_mock_conn(stream: &mut std::net::TcpStream) -> std::io::Result<()> {
        use std::io::{Read, Write};
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let mut buf = vec![0u8; 65536];
        let n = stream.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        let req = String::from_utf8_lossy(&buf[..n]);
        let first = req.lines().next().unwrap_or("");
        let body = if first.starts_with("POST /_bulk") {
            r#"{"errors":false,"items":[{"index":{"status":201}}]}"#
        } else if first.contains("/_search") {
            r#"{"hits":{"total":{"value":1},"hits":[{"_score":1.0,"_source":{"title":"a","path":"/tmp/a.pdf","page":1,"text":"hello world"},"highlight":{"text":["【hello】 world"]}}]}}"#
        } else if first.starts_with("PUT ") {
            r#"{"acknowledged":true}"#
        } else {
            ""
        };
        let status = if first.starts_with("HEAD ") {
            "HTTP/1.1 404 Not Found"
        } else {
            "HTTP/1.1 200 OK"
        };
        let resp = format!(
            "{status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(resp.as_bytes())?;
        Ok(())
    }
}
