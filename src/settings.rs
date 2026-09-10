use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// OpenSearch 接続設定（`setting.toml` の `[opensearch]` セクション）。
#[derive(Debug, Clone, Deserialize)]
pub struct OpenSearchSettings {
    /// 接続 URL。指定時は `scheme` / `host` / `port` より優先。
    #[serde(default, alias = "base_url", alias = "endpoint")]
    pub url: String,
    #[serde(default = "default_host")]
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    #[serde(default = "default_scheme")]
    pub scheme: String,
    #[serde(default = "default_index")]
    pub index: String,
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub password: String,
    /// 設定されている場合は Basic 認証より優先（`Authorization: ApiKey …`）。
    #[serde(default)]
    pub api_key: String,
    /// 自己署名証明書など、TLS 検証をスキップする。
    #[serde(default, alias = "skip_tls_verify", alias = "tls_insecure")]
    pub insecure_skip_tls_verify: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct SettingsFile {
    opensearch: OpenSearchSettings,
}

fn default_host() -> String {
    "localhost".into()
}

fn default_port() -> u16 {
    9200
}

fn default_scheme() -> String {
    "https".into()
}

fn default_index() -> String {
    "pdf_docs".into()
}

impl OpenSearchSettings {
    pub fn base_url(&self) -> String {
        let url = self.url.trim().trim_end_matches('/');
        if !url.is_empty() {
            return url.to_string();
        }
        let scheme = self.scheme.trim();
        let scheme = if scheme.is_empty() { "https" } else { scheme };
        format!("{}://{}:{}", scheme, self.host.trim(), self.port)
    }

    pub fn index(&self) -> &str {
        let index = self.index.trim();
        if index.is_empty() { "pdf_docs" } else { index }
    }

    pub fn auth_label(&self) -> &'static str {
        if !self.api_key.trim().is_empty() {
            "APIキー"
        } else if !self.username.trim().is_empty() {
            "ユーザー名/パスワード"
        } else {
            "なし"
        }
    }
}

/// `setting.toml` の探索順: 実行ファイルと同じフォルダ → カレントディレクトリ。
pub fn find_setting_file() -> Option<PathBuf> {
    let mut seen = Vec::new();
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        seen.push(dir.join("setting.toml"));
    }
    if let Ok(cwd) = std::env::current_dir() {
        seen.push(cwd.join("setting.toml"));
    }
    seen.into_iter().find(|path| path.is_file())
}

pub fn load_settings() -> Result<(PathBuf, OpenSearchSettings), String> {
    let path = find_setting_file().ok_or_else(|| {
        "setting.toml が見つかりません。実行ファイルと同じフォルダ、または作業ディレクトリに \
         setting.toml.example をコピーして setting.toml を作成してください。"
            .to_string()
    })?;
    load_settings_from(&path).map(|settings| (path, settings))
}

pub fn load_settings_from(path: &Path) -> Result<OpenSearchSettings, String> {
    let raw = fs::read_to_string(path)
        .map_err(|err| format!("setting.toml を読めません ({}): {err}", path.display()))?;
    let file: SettingsFile = toml::from_str(&raw).map_err(|err| {
        format!(
            "setting.toml の解析に失敗しました ({}): {err}",
            path.display()
        )
    })?;
    Ok(file.opensearch)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url_and_aliases() {
        let toml = r#"
            [opensearch]
            url = "https://opensearch.example:9200/"
            index = "papers"
            username = "admin"
            password = "secret"
            skip_tls_verify = true
        "#;
        let file: SettingsFile = toml::from_str(toml).unwrap();
        assert_eq!(
            file.opensearch.base_url(),
            "https://opensearch.example:9200"
        );
        assert_eq!(file.opensearch.index(), "papers");
        assert!(file.opensearch.insecure_skip_tls_verify);
        assert_eq!(file.opensearch.auth_label(), "ユーザー名/パスワード");
    }

    #[test]
    fn builds_url_from_host_port() {
        let toml = r#"
            [opensearch]
            host = "127.0.0.1"
            port = 9201
            scheme = "http"
        "#;
        let file: SettingsFile = toml::from_str(toml).unwrap();
        assert_eq!(file.opensearch.base_url(), "http://127.0.0.1:9201");
        assert_eq!(file.opensearch.index(), "pdf_docs");
        assert_eq!(file.opensearch.auth_label(), "なし");
    }

    #[test]
    fn api_key_auth_label() {
        let toml = r#"
            [opensearch]
            api_key = "abcd"
        "#;
        let file: SettingsFile = toml::from_str(toml).unwrap();
        assert_eq!(file.opensearch.auth_label(), "APIキー");
    }
}
