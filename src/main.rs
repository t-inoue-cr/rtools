use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use eframe::egui;

enum ExportState {
    Idle,
    PickFolder,
    Listing,
    PickSave { names: Vec<String> },
    Writing,
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([720.0, 480.0])
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
    const CANDIDATES: &[(&str, u32)] = &[
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
    for &(name, index) in CANDIDATES {
        if let Ok(bytes) = fs::read(fonts_dir.join(name)) {
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

struct RToolsApp {
    log: String,
    #[allow(dead_code)] // kept so workers can log via the background thread
    log_tx: Sender<String>,
    log_buffer: Arc<Mutex<String>>,
    log_follow_bottom: bool,
    log_len_shown: usize,
    export: ExportState,
    yield_before_action: bool,
    list_rx: Option<Receiver<Result<Vec<String>, String>>>,
    write_rx: Option<Receiver<Result<PathBuf, String>>>,
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
            yield_before_action: false,
            list_rx: None,
            write_rx: None,
        };
        app.log_line("rtools を起動しました。");
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
        if let Ok(buffer) = self.log_buffer.lock() {
            if buffer.len() > self.log.len() {
                self.log.clone_from(&buffer);
            }
        }
    }

    fn is_idle(&self) -> bool {
        matches!(self.export, ExportState::Idle)
    }

    fn start_export(&mut self) {
        self.log_line("フォルダ選択ダイアログを開きます。");
        self.export = ExportState::PickFolder;
        self.yield_before_action = true;
    }

    fn poll_export(&mut self, ctx: &egui::Context) {
        if self.yield_before_action {
            self.yield_before_action = false;
            ctx.request_repaint();
            return;
        }

        if matches!(self.export, ExportState::Idle) {
            return;
        }
        if matches!(self.export, ExportState::PickFolder) {
            self.pick_folder();
            return;
        }
        if matches!(self.export, ExportState::Listing) {
            self.poll_listing(ctx);
            return;
        }
        if matches!(self.export, ExportState::PickSave { .. }) {
            self.pick_save();
            return;
        }
        if matches!(self.export, ExportState::Writing) {
            self.poll_writing(ctx);
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
}

impl eframe::App for RToolsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.sync_log();
        self.poll_export(ctx);
        self.sync_log();

        let idle = self.is_idle();
        let follow_bottom = self.log_follow_bottom;

        egui::TopBottomPanel::top("toolbar")
            .min_height(56.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let button = egui::Button::new("フォルダ内のファイル一覧を保存")
                        .min_size(egui::vec2(0.0, 40.0))
                        .corner_radius(6.0)
                        .fill(egui::Color32::from_rgb(232, 240, 254));
                    if ui.add_enabled(idle, button).clicked() {
                        self.start_export();
                    }
                });
            });

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::Frame::new()
                .fill(ui.visuals().extreme_bg_color)
                .stroke(egui::Stroke::new(1.5_f32, egui::Color32::from_gray(80)))
                .corner_radius(6.0)
                .inner_margin(egui::Margin::symmetric(8, 8))
                .show(ui, |ui| {
                    let log_grew = self.log.len() != self.log_len_shown;
                    let output = egui::ScrollArea::vertical()
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
                    let max_offset = (output.content_size.y - output.inner_rect.height()).max(0.0);
                    let at_bottom = output.state.offset.y >= max_offset - 1.0;
                    self.log_follow_bottom = if log_grew && follow_bottom {
                        true
                    } else {
                        at_bottom
                    };
                });
        });

        self.log_len_shown = self.log.len();
    }
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
