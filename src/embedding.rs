#[cfg(not(test))]
use std::path::PathBuf;

/// multilingual-e5-small の出力次元。OpenSearch の `knn_vector` と一致させる。
pub const EMBEDDING_DIM: usize = 384;

const QUERY_PREFIX: &str = "query: ";
const PASSAGE_PREFIX: &str = "passage: ";

pub fn with_query_prefix(text: &str) -> String {
    format!("{QUERY_PREFIX}{}", text.trim())
}

pub fn with_passage_prefix(text: &str) -> String {
    format!("{PASSAGE_PREFIX}{}", text.trim())
}

/// 検索文を埋め込む（E5 の `query: ` プレフィックス付き）。
pub fn embed_query(text: &str) -> Result<Vec<f32>, String> {
    let input = with_query_prefix(text);
    if input == QUERY_PREFIX {
        return Err("検索文が空です。".into());
    }
    let mut vectors = embed_batch(std::slice::from_ref(&input))?;
    let vector = vectors
        .pop()
        .ok_or_else(|| "埋め込み結果が空です。".to_string())?;
    check_dim(&vector)?;
    Ok(vector)
}

/// 登録用チャンクを埋め込む（E5 の `passage: ` プレフィックス付き）。
pub fn embed_passages<S: AsRef<str>>(texts: &[S]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    let inputs: Vec<String> = texts
        .iter()
        .map(|text| with_passage_prefix(text.as_ref()))
        .collect();
    let vectors = embed_batch(&inputs)?;
    if vectors.len() != texts.len() {
        return Err(format!(
            "埋め込み件数が一致しません（入力 {} / 出力 {}）。",
            texts.len(),
            vectors.len()
        ));
    }
    for vector in &vectors {
        check_dim(vector)?;
    }
    Ok(vectors)
}

fn check_dim(vector: &[f32]) -> Result<(), String> {
    if vector.len() == EMBEDDING_DIM {
        Ok(())
    } else {
        Err(format!(
            "埋め込み次元が {EMBEDDING_DIM} ではありません（{}）。",
            vector.len()
        ))
    }
}

fn embed_batch(texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    if texts.is_empty() {
        return Ok(Vec::new());
    }
    embed_batch_impl(texts)
}

#[cfg(test)]
fn embed_batch_impl(texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    Ok(texts
        .iter()
        .map(|_| vec![0.01_f32; EMBEDDING_DIM])
        .collect())
}

#[cfg(not(test))]
fn embed_batch_impl(texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
    use std::sync::Mutex;

    static MODEL: Mutex<Option<fastembed::TextEmbedding>> = Mutex::new(None);
    let mut guard = MODEL
        .lock()
        .map_err(|_| "埋め込みモデルのロックに失敗しました。".to_string())?;
    if guard.is_none() {
        *guard = Some(load_model()?);
    }
    guard
        .as_mut()
        .ok_or_else(|| "埋め込みモデルの初期化に失敗しました。".to_string())?
        .embed(texts, None)
        .map_err(|err| format!("埋め込みの計算に失敗しました: {err}"))
}

#[cfg(not(test))]
fn load_model() -> Result<fastembed::TextEmbedding, String> {
    use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

    let cache_dir = model_cache_dir();
    std::fs::create_dir_all(&cache_dir).map_err(|err| {
        format!(
            "埋め込みモデルのキャッシュフォルダを作成できません ({}): {err}",
            cache_dir.display()
        )
    })?;
    TextEmbedding::try_new(
        TextInitOptions::new(EmbeddingModel::MultilingualE5Small)
            .with_cache_dir(cache_dir)
            .with_show_download_progress(true),
    )
    .map_err(|err| format!("埋め込みモデルの読み込みに失敗しました: {err}"))
}

#[cfg(not(test))]
fn model_cache_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("LOCALAPPDATA") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join("rtools").join("models");
        }
    }
    if let Ok(dir) = std::env::var("XDG_CACHE_HOME") {
        let dir = dir.trim();
        if !dir.is_empty() {
            return PathBuf::from(dir).join("rtools").join("models");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".cache")
            .join("rtools")
            .join("models");
    }
    std::env::temp_dir().join("rtools").join("models")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefixes_query_and_passage() {
        assert_eq!(with_query_prefix("  こんにちは  "), "query: こんにちは");
        assert_eq!(with_passage_prefix("hello"), "passage: hello");
    }

    #[test]
    fn embed_query_rejects_blank() {
        assert!(embed_query("   ").is_err());
    }

    #[test]
    fn embed_passages_match_input_len_and_dim() {
        let vectors = embed_passages(&["a", "b"]).unwrap();
        assert_eq!(vectors.len(), 2);
        assert_eq!(vectors[0].len(), EMBEDDING_DIM);
        assert!(embed_passages(&[] as &[&str]).unwrap().is_empty());
    }
}
