# rtools

Windows 向けの小さなデスクトップツール（eframe / egui）です。フォルダ内のファイル一覧保存に加え、PDF を OpenSearch へ登録してキーワード検索できます。Linux では日本語フォントのフォールバックと GTK 3 のフォルダ選択を使います（xdg-desktop-portal は使いません）。Windows のファイルダイアログは従来どおりネイティブです。

## 設定

`setting.toml.example` を `setting.toml` にコピーし、OpenSearch の接続情報を記入します。アプリは次の順でファイルを探します。

1. 実行ファイルと同じフォルダの `setting.toml`
2. カレントディレクトリの `setting.toml`

```toml
[opensearch]
url = "https://localhost:9200"   # 優先。空なら host/port/scheme から組み立て
# host = "localhost"
# port = 9200
# scheme = "https"
index = "pdf_docs"
username = "admin"
password = "changeme"
# api_key = ""                   # 指定時は Basic 認証より優先（Authorization: ApiKey）
insecure_skip_tls_verify = true  # 自己署名証明書向け。本番では false を推奨
```

`setting.toml` は `.gitignore` 済みです。秘密情報をリポジトリに含めないでください。

OpenSearch 本体の起動や Docker 化はこのアプリの対象外です。あらかじめ稼働しているクラスタへクライアントとして接続します。

## 使い方

- **フォルダ内のファイル一覧を保存** — 従来どおり、選択フォルダ直下のファイル名をテキストに書き出します。
- **PDFをOpenSearchに登録** — PDF が入ったフォルダを選び、再帰的に `.pdf` を集めてテキスト化してインデックスします。
- **検索** — キーワードを入れて検索し、ヒットを画面下部に表示します。

PDF は純 Rust の `pdf_oxide` でテキスト抽出します。日本語特許 PDF（OpenPDF / JPO、UniJIS-UCS2-H など）もこのクレートで扱います。ページごとに抽出し、フォームフィードでつなぐので、ページ単位のチャンク分割が使えます。ページ区切りが無い場合は約 1800 文字のチャンクに分割して登録します（フィールド: `title`, `file_name`, `path`, `page`, `chunk`, `text`, `ingested_at`）。スキャン画像のみの PDF は OCR しないため、プレースホルダ文言だけが登録されることがあります。
