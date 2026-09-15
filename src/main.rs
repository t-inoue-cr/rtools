mod embedding;
mod opensearch;
mod pdf;
mod pdf_layout;
mod settings;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui;

use opensearch::{IngestReport, SearchHit, SearchOutcome, snippet_segments};
use pdf::{collect_pdfs, markdown_file_name, pdf_to_markdown};
use settings::OpenSearchSettings;

enum ExportState {
    Idle,
    PickFolder,
    Listing,
    PickSave { names: Vec<String> },
    Writing,
}

enum IngestState {
    Idle,
    PickFolder,
    Running,
}

enum ConvertState {
    Idle,
    PickPdf,
    Converting {
        file_name: String,
        rx: Receiver<Result<String, String>>,
    },
    PickSave {
        markdown: String,
        file_name: String,
    },
    Writing(Receiver<Result<PathBuf, String>>),
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([880.0, 800.0])
            .with_title("rtools"),
        ..Default::default()
    };

    eframe::run_native(
        "rtools",
        options,
        Box::new(|cc| {
            install_japanese_fonts(&cc.egui_ctx);
            cc.egui_ctx.set_theme(egui::ThemePreference::Light);
            apply_ui_style(&cc.egui_ctx);
            Ok(Box::new(RToolsApp::new(&cc.egui_ctx)))
        }),
    )
}

fn windows_fonts_dir() -> PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(windir).join("Fonts")
}

fn load_japanese_font() -> Option<egui::FontData> {
    // TTC collections include both document faces (index 0) and UI faces.
    // Document 游ゴシック has extra descent, so button text looks too high.
    const WINDOWS_CANDIDATES: &[(&str, u32)] = &[
        ("YuGothM.ttc", 1),  // Yu Gothic UI Regular
        ("YuGothB.ttc", 2),  // Yu Gothic UI Semibold
        ("YuGothR.ttc", 1),  // Yu Gothic UI Semilight
        ("meiryo.ttc", 2),   // Meiryo UI
        ("msgothic.ttc", 1), // MS UI Gothic
        ("YuGothM.ttc", 0),
        ("meiryo.ttc", 0),
        ("YuGothR.ttc", 0),
        ("msgothic.ttc", 0),
    ];
    let fonts_dir = windows_fonts_dir();
    for &(name, index) in WINDOWS_CANDIDATES {
        if let Ok(bytes) = fs::read(fonts_dir.join(name)) {
            let mut font = egui::FontData::from_owned(bytes);
            font.index = index;
            return Some(font);
        }
    }

    const LINUX_CANDIDATES: &[(&str, u32)] = &[
        ("/usr/share/fonts/truetype/wqy/wqy-microhei.ttc", 0),
        (
            "/usr/share/fonts/truetype/droid/DroidSansFallbackFull.ttf",
            0,
        ),
        ("/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc", 0),
        ("/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc", 0),
    ];
    for &(path, index) in LINUX_CANDIDATES {
        if let Ok(bytes) = fs::read(path) {
            let mut font = egui::FontData::from_owned(bytes);
            font.index = index;
            return Some(font);
        }
    }
    None
}

fn apply_ui_style(ctx: &egui::Context) {
    ctx.all_styles_mut(|style| {
        style.spacing.button_padding = egui::vec2(16.0, 10.0);
        style.spacing.item_spacing = egui::vec2(12.0, 10.0);
        style.spacing.interact_size.y = 40.0;

        style.text_styles.insert(
            egui::TextStyle::Button,
            egui::FontId::new(16.0, egui::FontFamily::Proportional),
        );

        let text = egui::Color32::from_gray(20);
        let widgets = &mut style.visuals.widgets;
        widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        widgets.inactive.fg_stroke = egui::Stroke::new(1.0_f32, text);
        widgets.hovered.fg_stroke = egui::Stroke::new(1.5_f32, text);
        widgets.active.fg_stroke = egui::Stroke::new(2.0_f32, text);

        widgets.inactive.weak_bg_fill = egui::Color32::WHITE;
        widgets.inactive.bg_fill = egui::Color32::from_gray(240);
        widgets.inactive.bg_stroke = egui::Stroke::NONE;
        widgets.inactive.corner_radius = egui::CornerRadius::same(6);

        widgets.hovered.weak_bg_fill = egui::Color32::from_gray(235);
        widgets.hovered.bg_fill = egui::Color32::from_gray(225);
        widgets.hovered.bg_stroke =
            egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(40, 80, 160));
        widgets.hovered.corner_radius = egui::CornerRadius::same(6);

        widgets.active.weak_bg_fill = egui::Color32::from_gray(210);
        widgets.active.bg_fill = egui::Color32::from_gray(200);
        widgets.active.bg_stroke = egui::Stroke::new(2.0_f32, egui::Color32::BLACK);
        widgets.active.corner_radius = egui::CornerRadius::same(6);
    });
}

fn install_japanese_fonts(ctx: &egui::Context) {
    let Some(font_data) = load_japanese_font() else {
        return;
    };

    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert("jp".to_owned(), Arc::new(font_data));
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .insert(0, "jp".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .insert(0, "jp".to_owned());
    ctx.set_fonts(fonts);
}

#[derive(Clone, Copy)]
enum SearchKind {
    Keyword,
    Sentence,
}

impl SearchKind {
    fn label(self) -> &'static str {
        match self {
            Self::Keyword => "キーワード検索（BM25）",
            Self::Sentence => "文章検索（kNN）",
        }
    }
}

struct RToolsApp {
    log: String,
    log_tx: Sender<String>,
    log_buffer: Arc<Mutex<String>>,
    log_follow_bottom: bool,
    log_len_shown: usize,
    export: ExportState,
    ingest: IngestState,
    convert: ConvertState,
    yield_before_action: bool,
    list_rx: Option<Receiver<Result<Vec<String>, String>>>,
    write_rx: Option<Receiver<Result<PathBuf, String>>>,
    ingest_rx: Option<Receiver<Result<IngestReport, String>>>,
    search_rx: Option<Receiver<Result<SearchOutcome, String>>>,
    search_kind: SearchKind,
    search_query: String,
    sentence_query: String,
    search_hits: Vec<SearchHit>,
    search_max_score: f64,
    search_status: String,
}

impl RToolsApp {
    fn new(ctx: &egui::Context) -> Self {
        let (log_tx, log_rx) = mpsc::channel::<String>();
        let log_buffer = Arc::new(Mutex::new(String::new()));
        let buffer = log_buffer.clone();
        let ctx = ctx.clone();

        std::thread::Builder::new()
            .name("log".into())
            .spawn(move || {
                while let Ok(msg) = log_rx.recv() {
                    let line = format_log_line(&msg);
                    if let Ok(mut log) = buffer.lock() {
                        log.push_str(&line);
                    }
                    ctx.request_repaint();
                }
            })
            .expect("failed to spawn log thread");

        let mut app = Self {
            log: String::new(),
            log_tx,
            log_buffer,
            log_follow_bottom: true,
            log_len_shown: 0,
            export: ExportState::Idle,
            ingest: IngestState::Idle,
            convert: ConvertState::Idle,
            yield_before_action: false,
            list_rx: None,
            write_rx: None,
            ingest_rx: None,
            search_rx: None,
            search_kind: SearchKind::Keyword,
            search_query: String::new(),
            sentence_query: String::new(),
            search_hits: Vec::new(),
            search_max_score: 0.0,
            search_status: "検索結果はここに表示されます。".into(),
        };
        app.log_line("rtools を起動しました。");
        match settings::load_settings() {
            Ok((path, cfg)) => {
                app.log_line(format!(
                    "OpenSearch 設定を読み込みました: {}  index={}  認証={}  ({})",
                    cfg.base_url(),
                    cfg.index(),
                    cfg.auth_label(),
                    path.display()
                ));
            }
            Err(err) => app.log_line(err),
        }
        app
    }

    fn log_line(&mut self, line: impl AsRef<str>) {
        let line = format_log_line(line.as_ref());
        self.log.push_str(&line);
        if let Ok(mut buffer) = self.log_buffer.lock() {
            buffer.push_str(&line);
        }
    }

    fn sync_log(&mut self) {
        if let Ok(buffer) = self.log_buffer.lock()
            && buffer.len() > self.log.len()
        {
            self.log.clone_from(&buffer);
        }
    }

    fn is_idle(&self) -> bool {
        matches!(self.export, ExportState::Idle)
            && matches!(self.ingest, IngestState::Idle)
            && matches!(self.convert, ConvertState::Idle)
            && self.search_rx.is_none()
    }

    fn busy_hint(&self) -> Option<&'static str> {
        if !matches!(self.export, ExportState::Idle) {
            Some("ファイル一覧を処理中…")
        } else if !matches!(self.ingest, IngestState::Idle) {
            Some("PDFを登録中…")
        } else if !matches!(self.convert, ConvertState::Idle) {
            Some("PDFを変換中…")
        } else if self.search_rx.is_some() {
            Some("検索中…")
        } else {
            None
        }
    }

    fn start_export(&mut self) {
        self.log_line("フォルダ選択ダイアログを開きます。");
        self.export = ExportState::PickFolder;
        self.yield_before_action = true;
    }

    fn start_ingest(&mut self) {
        self.log_line("PDF フォルダの選択ダイアログを開きます。");
        self.ingest = IngestState::PickFolder;
        self.yield_before_action = true;
    }

    fn start_convert(&mut self) {
        self.log_line("PDF の選択ダイアログを開きます。");
        self.convert = ConvertState::PickPdf;
        self.yield_before_action = true;
    }

    fn start_search(&mut self, kind: SearchKind, query: String) {
        let query = query.trim().to_string();
        if query.is_empty() {
            let msg = match kind {
                SearchKind::Keyword => "検索キーワードを入力してください。",
                SearchKind::Sentence => "検索する文章を入力してください。",
            };
            self.log_line(msg);
            return;
        }

        let settings = match load_opensearch_or_log(&mut self.log, &self.log_buffer) {
            Some(settings) => settings,
            None => return,
        };

        self.search_kind = kind;
        let preview = search_query_preview(&query);
        self.log_line(format!("{}しています: {preview}", kind.label()));
        if matches!(kind, SearchKind::Sentence) {
            self.log_line(
                "埋め込みモデルを準備しています（初回は約 120MB のダウンロードがあります）。",
            );
        }
        self.search_status = "検索中…".into();
        let (tx, rx) = mpsc::channel();
        self.search_rx = Some(rx);
        let log_tx = self.log_tx.clone();

        std::thread::Builder::new()
            .name("opensearch-search".into())
            .spawn(move || {
                let result = match kind {
                    SearchKind::Keyword => opensearch::search(&settings, &query),
                    SearchKind::Sentence => opensearch::knn_search(&settings, &query),
                };
                if let Err(err) = &result {
                    let _ = log_tx.send(format!("検索に失敗しました: {err}"));
                }
                let _ = tx.send(result);
            })
            .expect("failed to spawn search thread");
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        if matches!(self.export, ExportState::Idle) {
            return;
        }
        if matches!(self.export, ExportState::PickFolder) {
            if self.yield_before_action {
                return;
            }
            self.pick_folder();
            return;
        }
        if matches!(self.export, ExportState::Listing) {
            self.poll_listing(ctx);
            return;
        }
        if matches!(self.export, ExportState::PickSave { .. }) {
            if self.yield_before_action {
                return;
            }
            self.pick_save();
            return;
        }
        if matches!(self.export, ExportState::Writing) {
            self.poll_writing(ctx);
        }
    }

    fn poll_ingest(&mut self, ctx: &egui::Context) {
        if matches!(self.ingest, IngestState::Idle) {
            return;
        }
        if matches!(self.ingest, IngestState::PickFolder) {
            if self.yield_before_action {
                return;
            }
            self.pick_ingest_folder();
            return;
        }
        if matches!(self.ingest, IngestState::Running) {
            self.poll_ingest_worker(ctx);
        }
    }

    fn poll_convert(&mut self, ctx: &egui::Context) {
        match &self.convert {
            ConvertState::Idle => {}
            ConvertState::PickPdf => {
                if !self.yield_before_action {
                    self.pick_pdf();
                }
            }
            ConvertState::Converting { .. } => self.poll_converting(ctx),
            ConvertState::PickSave { .. } => {
                if !self.yield_before_action {
                    self.pick_convert_save();
                }
            }
            ConvertState::Writing(_) => self.poll_convert_writing(ctx),
        }
    }

    fn poll_search(&mut self, ctx: &egui::Context) {
        let recv = {
            let Some(rx) = &self.search_rx else {
                return;
            };
            rx.try_recv()
        };

        match recv {
            Ok(Ok(outcome)) => {
                self.search_rx = None;
                self.search_max_score = outcome.max_score;
                self.search_hits = outcome.hits;
                self.search_status = format!(
                    "{}: {} 件ヒット（表示 {} 件）",
                    self.search_kind.label(),
                    outcome.total,
                    self.search_hits.len()
                );
                self.log_line(self.search_status.clone());
            }
            Ok(Err(err)) => {
                self.search_rx = None;
                self.search_hits.clear();
                self.search_max_score = 0.0;
                self.search_status = format!("検索に失敗しました: {err}");
                self.log_line(self.search_status.clone());
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.search_rx = None;
                self.search_hits.clear();
                self.search_max_score = 0.0;
                self.search_status = "検索に失敗しました: ワーカーが終了しました。".into();
                self.log_line(self.search_status.clone());
            }
        }
    }

    fn pick_folder(&mut self) {
        self.export = ExportState::Idle;
        let Some(folder) = rfd::FileDialog::new()
            .set_title("フォルダを選択")
            .pick_folder()
        else {
            self.log_line("フォルダ選択をキャンセルしました。");
            return;
        };

        self.log_line(format!("フォルダ: {}", folder.display()));
        self.log_line("ファイル一覧を取得しています。");
        self.start_listing(folder);
    }

    fn pick_ingest_folder(&mut self) {
        self.ingest = IngestState::Idle;
        let Some(folder) = rfd::FileDialog::new()
            .set_title("PDFが入ったフォルダを選択")
            .pick_folder()
        else {
            self.log_line("PDF フォルダの選択をキャンセルしました。");
            return;
        };

        self.log_line(format!("PDF フォルダ: {}", folder.display()));

        let Some(settings) = load_opensearch_or_log(&mut self.log, &self.log_buffer) else {
            return;
        };

        self.start_ingest_worker(settings, folder);
    }

    fn start_ingest_worker(&mut self, settings: OpenSearchSettings, folder: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.ingest_rx = Some(rx);
        self.ingest = IngestState::Running;
        let log_tx = self.log_tx.clone();

        std::thread::Builder::new()
            .name("opensearch-ingest".into())
            .spawn(move || {
                let result = (|| {
                    let _ = log_tx.send("PDF ファイルを列挙しています。".into());
                    let pdfs = collect_pdfs(&folder)?;
                    if pdfs.is_empty() {
                        return Err("フォルダ内に PDF ファイルが見つかりませんでした。".into());
                    }
                    let _ = log_tx.send(format!("PDF ファイル数: {}", pdfs.len()));
                    for path in pdfs.iter().take(20) {
                        let _ = log_tx.send(format!("  - {}", path.display()));
                    }
                    if pdfs.len() > 20 {
                        let _ = log_tx.send(format!("  …ほか {} 件", pdfs.len() - 20));
                    }
                    opensearch::ingest_files(&settings, &pdfs, &log_tx)
                })();
                let _ = tx.send(result);
            })
            .expect("failed to spawn ingest thread");
    }

    fn poll_ingest_worker(&mut self, ctx: &egui::Context) {
        let recv = {
            let Some(rx) = &self.ingest_rx else {
                self.ingest = IngestState::Idle;
                return;
            };
            rx.try_recv()
        };

        match recv {
            Ok(Ok(report)) => {
                self.ingest_rx = None;
                self.ingest = IngestState::Idle;
                self.log_line(format!(
                    "登録が完了しました: 成功 {} ファイル / 失敗 {} / {} チャンク",
                    report.files_ok, report.files_fail, report.chunks
                ));
                for err in report.errors.iter().take(8) {
                    self.log_line(format!("  警告: {err}"));
                }
                if report.errors.len() > 8 {
                    self.log_line(format!("  …ほか {} 件の警告", report.errors.len() - 8));
                }
            }
            Ok(Err(err)) => {
                self.ingest_rx = None;
                self.ingest = IngestState::Idle;
                self.log_line(format!("登録に失敗しました: {err}"));
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.ingest_rx = None;
                self.ingest = IngestState::Idle;
                self.log_line("登録に失敗しました: ワーカーが終了しました。");
            }
        }
    }

    fn start_listing(&mut self, folder: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.list_rx = Some(rx);
        self.export = ExportState::Listing;

        std::thread::Builder::new()
            .name("list-files".into())
            .spawn(move || {
                let result = list_files_in_folder(&folder).map_err(|err| err.to_string());
                let _ = tx.send(result);
            })
            .expect("failed to spawn list-files thread");
    }

    fn poll_listing(&mut self, ctx: &egui::Context) {
        let recv = {
            let Some(rx) = &self.list_rx else {
                self.export = ExportState::Idle;
                return;
            };
            rx.try_recv()
        };

        match recv {
            Ok(Ok(names)) => {
                self.list_rx = None;
                self.log_line(format!("ファイル数: {}", names.len()));
                self.log_line("保存先の選択ダイアログを開きます。");
                self.export = ExportState::PickSave { names };
                self.yield_before_action = true;
            }
            Ok(Err(err)) => {
                self.list_rx = None;
                self.log_line(format!("ファイル一覧の取得に失敗しました: {err}"));
                self.export = ExportState::Idle;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.list_rx = None;
                self.log_line("ファイル一覧の取得に失敗しました: ワーカーが終了しました。");
                self.export = ExportState::Idle;
            }
        }
    }

    fn pick_save(&mut self) {
        let ExportState::PickSave { names } =
            std::mem::replace(&mut self.export, ExportState::Idle)
        else {
            return;
        };

        let Some(save_path) = rfd::FileDialog::new()
            .set_title("ファイル一覧を保存")
            .add_filter("テキスト", &["txt"])
            .set_file_name("file_list.txt")
            .save_file()
        else {
            self.log_line("保存をキャンセルしました。");
            return;
        };

        self.log_line(format!("保存しています: {}", save_path.display()));
        self.start_write(names, save_path);
    }

    fn start_write(&mut self, names: Vec<String>, save_path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.write_rx = Some(rx);
        self.export = ExportState::Writing;

        std::thread::Builder::new()
            .name("write-file".into())
            .spawn(move || {
                let contents = names.join("\n");
                let result = fs::write(&save_path, contents)
                    .map(|()| save_path)
                    .map_err(|err| err.to_string());
                let _ = tx.send(result);
            })
            .expect("failed to spawn write-file thread");
    }

    fn poll_writing(&mut self, ctx: &egui::Context) {
        let recv = {
            let Some(rx) = &self.write_rx else {
                self.export = ExportState::Idle;
                return;
            };
            rx.try_recv()
        };

        match recv {
            Ok(Ok(save_path)) => {
                self.write_rx = None;
                self.log_line(format!("保存しました: {}", save_path.display()));
                self.export = ExportState::Idle;
            }
            Ok(Err(err)) => {
                self.write_rx = None;
                self.log_line(format!("保存に失敗しました: {err}"));
                self.export = ExportState::Idle;
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.write_rx = None;
                self.log_line("保存に失敗しました: ワーカーが終了しました。");
                self.export = ExportState::Idle;
            }
        }
    }

    fn pick_pdf(&mut self) {
        self.convert = ConvertState::Idle;
        let Some(pdf_path) = rfd::FileDialog::new()
            .set_title("PDFを選択")
            .add_filter("PDF", &["pdf"])
            .pick_file()
        else {
            self.log_line("PDF の選択をキャンセルしました。");
            return;
        };

        self.log_line(format!("PDF: {}", pdf_path.display()));
        self.log_line("Markdown に変換しています。");
        self.start_converting(pdf_path);
    }

    fn start_converting(&mut self, pdf_path: PathBuf) {
        let file_name = markdown_file_name(&pdf_path);
        let (tx, rx) = mpsc::channel();
        self.convert = ConvertState::Converting { file_name, rx };

        std::thread::Builder::new()
            .name("pdf-to-markdown".into())
            .spawn(move || {
                let _ = tx.send(pdf_to_markdown(&pdf_path));
            })
            .expect("failed to spawn pdf-to-markdown thread");
    }

    fn poll_converting(&mut self, ctx: &egui::Context) {
        let recv = match &self.convert {
            ConvertState::Converting { rx, .. } => rx.try_recv(),
            _ => return,
        };

        match recv {
            Ok(result) => {
                let ConvertState::Converting { file_name, .. } =
                    std::mem::replace(&mut self.convert, ConvertState::Idle)
                else {
                    return;
                };
                match result {
                    Ok(markdown) => {
                        self.log_line("保存先の選択ダイアログを開きます。");
                        self.convert = ConvertState::PickSave {
                            markdown,
                            file_name,
                        };
                        self.yield_before_action = true;
                    }
                    Err(err) => {
                        self.log_line(format!("PDF の変換に失敗しました: {err}"));
                    }
                }
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.convert = ConvertState::Idle;
                self.log_line("PDF の変換に失敗しました: ワーカーが終了しました。");
            }
        }
    }

    fn pick_convert_save(&mut self) {
        let ConvertState::PickSave {
            markdown,
            file_name,
        } = std::mem::replace(&mut self.convert, ConvertState::Idle)
        else {
            return;
        };

        let Some(save_path) = rfd::FileDialog::new()
            .set_title("Markdownを保存")
            .add_filter("Markdown", &["md"])
            .set_file_name(&file_name)
            .save_file()
        else {
            self.log_line("保存をキャンセルしました。");
            return;
        };

        self.log_line(format!("保存しています: {}", save_path.display()));
        self.start_convert_write(markdown, save_path);
    }

    fn start_convert_write(&mut self, markdown: String, save_path: PathBuf) {
        let (tx, rx) = mpsc::channel();
        self.convert = ConvertState::Writing(rx);

        std::thread::Builder::new()
            .name("write-markdown".into())
            .spawn(move || {
                let result = fs::write(&save_path, markdown)
                    .map(|()| save_path)
                    .map_err(|err| err.to_string());
                let _ = tx.send(result);
            })
            .expect("failed to spawn write-markdown thread");
    }

    fn poll_convert_writing(&mut self, ctx: &egui::Context) {
        let recv = match &self.convert {
            ConvertState::Writing(rx) => rx.try_recv(),
            _ => return,
        };

        match recv {
            Ok(Ok(save_path)) => {
                self.convert = ConvertState::Idle;
                self.log_line(format!("保存しました: {}", save_path.display()));
            }
            Ok(Err(err)) => {
                self.convert = ConvertState::Idle;
                self.log_line(format!("保存に失敗しました: {err}"));
            }
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint();
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.convert = ConvertState::Idle;
                self.log_line("保存に失敗しました: ワーカーが終了しました。");
            }
        }
    }
}

fn load_opensearch_or_log(
    log: &mut String,
    log_buffer: &Arc<Mutex<String>>,
) -> Option<OpenSearchSettings> {
    match settings::load_settings() {
        Ok((path, settings)) => {
            let line = format_log_line(&format!(
                "接続先: {}  index={}  ({})",
                settings.base_url(),
                settings.index(),
                path.display()
            ));
            log.push_str(&line);
            if let Ok(mut buffer) = log_buffer.lock() {
                buffer.push_str(&line);
            }
            Some(settings)
        }
        Err(err) => {
            let line = format_log_line(&err);
            log.push_str(&line);
            if let Ok(mut buffer) = log_buffer.lock() {
                buffer.push_str(&line);
            }
            None
        }
    }
}

impl eframe::App for RToolsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_log();
        if self.yield_before_action {
            self.yield_before_action = false;
            ctx.request_repaint();
        } else {
            self.poll_export(ctx);
            self.poll_ingest(ctx);
            self.poll_convert(ctx);
            self.poll_search(ctx);
        }
        self.sync_log();

        let idle = self.is_idle();
        let follow_bottom = self.log_follow_bottom;
        let busy_hint = self.busy_hint();
        let can_search = idle && !self.search_query.trim().is_empty();
        let can_sentence_search = idle && !self.sentence_query.trim().is_empty();

        egui::TopBottomPanel::top("toolbar")
            .min_height(248.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let export_button = egui::Button::new("フォルダ内のファイル一覧を保存")
                        .min_size(egui::vec2(0.0, 40.0))
                        .corner_radius(6.0)
                        .fill(egui::Color32::from_rgb(232, 240, 254));
                    if ui.add_enabled(idle, export_button).clicked() {
                        self.start_export();
                    }

                    let ingest_button = egui::Button::new("PDFをOpenSearchに登録")
                        .min_size(egui::vec2(0.0, 40.0))
                        .corner_radius(6.0)
                        .fill(egui::Color32::from_rgb(232, 240, 254));
                    if ui.add_enabled(idle, ingest_button).clicked() {
                        self.start_ingest();
                    }

                    let convert_button = egui::Button::new("PDFをMarkdownに変換")
                        .min_size(egui::vec2(0.0, 40.0))
                        .corner_radius(6.0)
                        .fill(egui::Color32::from_rgb(232, 240, 254));
                    if ui.add_enabled(idle, convert_button).clicked() {
                        self.start_convert();
                    }

                    if let Some(hint) = busy_hint {
                        ui.label(hint);
                    }
                });

                ui.add_space(4.0);
                ui.label(egui::RichText::new("キーワード検索（BM25）").strong());
                ui.weak("語句が一致する文書を探します。単語や短いフレーズを入力してください。");
                ui.horizontal(|ui| {
                    let edit = egui::TextEdit::singleline(&mut self.search_query)
                        .desired_width(420.0)
                        .hint_text("キーワードを入力")
                        .margin(egui::Margin {
                            left: 4,
                            right: 4,
                            top: 6,
                            bottom: 2,
                        })
                        .min_size(egui::vec2(0.0, 32.0));
                    let response = ui.add_enabled(idle, edit);
                    let enter =
                        response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));

                    let search_button = egui::Button::new("検索")
                        .min_size(egui::vec2(0.0, 40.0))
                        .corner_radius(6.0)
                        .fill(egui::Color32::from_rgb(232, 240, 254));
                    if ui.add_enabled(can_search, search_button).clicked() || (enter && can_search)
                    {
                        self.start_search(SearchKind::Keyword, self.search_query.clone());
                    }
                });

                ui.add_space(4.0);
                ui.label(egui::RichText::new("文章検索（ベクトル / kNN）").strong());
                ui.weak(
                    "意味が近い文書を探します。キーワードではなく、探したい内容を文章で入力してください。",
                );
                ui.horizontal(|ui| {
                    let edit = egui::TextEdit::multiline(&mut self.sentence_query)
                        .desired_width(420.0)
                        .desired_rows(3)
                        .hint_text("探したい内容を文章で入力");
                    ui.add_enabled(idle, edit);

                    let search_button = egui::Button::new("文章検索")
                        .min_size(egui::vec2(0.0, 40.0))
                        .corner_radius(6.0)
                        .fill(egui::Color32::from_rgb(232, 240, 254));
                    if ui.add_enabled(can_sentence_search, search_button).clicked() {
                        self.start_search(SearchKind::Sentence, self.sentence_query.clone());
                    }
                });
            });

        egui::TopBottomPanel::bottom("log")
            .resizable(true)
            .default_height(150.0)
            .min_height(80.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("ログ").strong());
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .stroke(egui::Stroke::new(1.5_f32, egui::Color32::from_gray(80)))
                    .corner_radius(6.0)
                    .inner_margin(egui::Margin::symmetric(8, 8))
                    .show(ui, |ui| {
                        let log_grew = self.log.len() != self.log_len_shown;
                        let output = egui::ScrollArea::vertical()
                            .id_salt("log")
                            .auto_shrink([false, false])
                            .stick_to_bottom(true)
                            .animated(false)
                            .show(ui, |ui| {
                                ui.add(egui::Label::new(&self.log).wrap().selectable(true));
                                if log_grew && follow_bottom {
                                    ui.allocate_response(egui::Vec2::ZERO, egui::Sense::hover())
                                        .scroll_to_me(Some(egui::Align::BOTTOM));
                                }
                            });
                        let max_offset =
                            (output.content_size.y - output.inner_rect.height()).max(0.0);
                        let at_bottom = output.state.offset.y >= max_offset - 1.0;
                        self.log_follow_bottom = if log_grew && follow_bottom {
                            true
                        } else {
                            at_bottom
                        };
                    });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(egui::RichText::new("検索結果").strong());
            ui.label(&self.search_status);
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(egui::Stroke::new(1.5_f32, egui::Color32::from_gray(80)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 8))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("search-hits")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if self.search_hits.is_empty() {
                                ui.label("ヒットはありません。");
                                return;
                            }
                            for hit in &self.search_hits {
                                ui.horizontal(|ui| {
                                    ui.label(similarity_label(hit.score, self.search_max_score));
                                    ui.strong(&hit.title);
                                    ui.label(format!("p.{}", hit.page));
                                });
                                if !hit.path.is_empty() {
                                    ui.weak(&hit.path);
                                }
                                if !hit.snippet.is_empty() {
                                    show_highlighted_snippet(ui, &hit.snippet);
                                }
                                ui.separator();
                            }
                        });
                });
        });

        self.log_len_shown = self.log.len();
    }
}

fn search_query_preview(query: &str) -> String {
    const MAX_CHARS: usize = 80;
    let mut chars = query.chars();
    let taken: String = chars.by_ref().take(MAX_CHARS).collect();
    if chars.next().is_some() {
        format!("{taken}…")
    } else {
        taken
    }
}

fn similarity_label(score: f64, max_score: f64) -> String {
    if max_score <= 0.0 {
        "類似度 —".into()
    } else {
        let pct = ((score / max_score) * 100.0).round() as i32;
        format!("類似度 {pct}%")
    }
}

fn show_highlighted_snippet(ui: &mut egui::Ui, snippet: &str) {
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = ui.available_width();
    let font_id = egui::TextStyle::Body.resolve(ui.style());
    let text_color = ui.visuals().text_color();
    let highlight = egui::Color32::from_rgb(255, 145, 0);
    for (is_hit, part) in snippet_segments(snippet) {
        let format = if is_hit {
            egui::text::TextFormat {
                font_id: font_id.clone(),
                color: egui::Color32::from_gray(20),
                background: highlight,
                ..Default::default()
            }
        } else {
            egui::text::TextFormat {
                font_id: font_id.clone(),
                color: text_color,
                ..Default::default()
            }
        };
        job.append(part, 0.0, format);
    }
    ui.add(egui::Label::new(job).wrap().selectable(true));
}

fn format_log_line(msg: &str) -> String {
    let ts = chrono::Local::now().format("[%Y-%m-%d %H:%M:%S]");
    format!("{ts} {msg}\n")
}

fn list_files_in_folder(folder: &Path) -> std::io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs::read_dir(folder)? {
        let entry = entry?;
        if entry.file_type()?.is_file() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}
