# rtools

Windows 向けの小さなデスクトップツール（eframe / egui）です。フォルダ内のファイル一覧保存に加え、PDF を OpenSearch へ登録して検索できます。検索は次の 2 種類です。

- **キーワード検索（BM25）** — 語句が一致する文書を探します。
- **文章検索（ベクトル / kNN）** — 入力した文章と意味が近い文書を探します。埋め込みはアプリ側の multilingual-e5-small（384 次元）で生成し、OpenSearch の kNN で近傍検索します。

Linux では日本語フォントのフォールバックと GTK 3 のフォルダ選択を使います（xdg-desktop-portal は使いません）。Windows のファイルダイアログは従来どおりネイティブです。

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

OpenSearch 本体の起動や Docker 化はこのアプリの対象外です。あらかじめ稼働しているクラスタへクライアントとして接続します。文章検索を使うには、クラスタ側で `knn_vector`（k-NN プラグイン）が使える必要があります。

文章検索の埋め込みモデルは初回利用時に Hugging Face から約 120MB 取得し、Windows では `%LOCALAPPDATA%\rtools\models`、Linux では `~/.cache/rtools/models` にキャッシュします。

## 使い方

- **フォルダ内のファイル一覧を保存** — 従来どおり、選択フォルダ直下のファイル名をテキストに書き出します。
- **PDFをOpenSearchに登録** — PDF が入ったフォルダを選び、再帰的に `.pdf` を集めてテキスト化してインデックスします。登録時に各チャンクのベクトルも保存します。
- **PDFをMarkdownに変換** — PDF を 1 つ選び、ファイル名をタイトルにした Markdown として保存します。ページ見出しは付けません。座標付きの行から折り返しを結合し、検出できた表は Markdown 表にします。スキャン画像のみの PDF は OCR しないため、変換できないことがあります。
- **文章変換** — PDF / Word / Excel / PowerPoint / HTML / Markdown / OpenDocument / RTF / EPUB / CSV などを 1 つ選び、`docling` の `DocumentConverter` だけで Markdown にします。自前のレイアウト処理は使いません。スキャン画像のみの PDF や画像 OCR、音声は対象外です。
- **キーワード検索（BM25）** — 単語や短いフレーズで検索し、ヒットを画面に表示します。
- **文章検索（ベクトル / kNN）** — 探したい内容を文章で入力して検索します。既存ドキュメントにベクトルが無い場合は、先に「PDFをOpenSearchに登録」をやり直してください。

PDF は純 Rust の `pdf_oxide` でテキスト抽出します。日本語特許 PDF（OpenPDF / JPO、UniJIS-UCS2-H など）もこのクレートで扱います。ページごとに座標付きの行（`extract_text_lines`）と表（`extract_tables`）を取り、見た目の折り返しは段落にまとめます。表は Markdown の pipe 表として書き出し、OpenSearch 登録時はセル文字列だけを本文に含めます。ページはフォームフィードでつなぐので、ページ単位のチャンク分割が使えます。ページ区切りが無い場合は約 1800 文字のチャンクに分割して登録します（フィールド: `title`, `file_name`, `path`, `page`, `chunk`, `text`, `ingested_at`, `embedding`）。スキャン画像のみの PDF は OCR しないため、プレースホルダ文言だけが登録されることがあります。罫線のない表は検出できないことがあります。
