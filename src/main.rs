use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use eframe::egui;

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
            Ok(Box::new(RToolsApp::default()))
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
        ("YuGothM.ttc", 1), // Yu Gothic UI Regular
        ("YuGothB.ttc", 2), // Yu Gothic UI Semibold
        ("YuGothR.ttc", 1), // Yu Gothic UI Semilight
        ("meiryo.ttc", 2),  // Meiryo UI
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
        widgets.hovered.bg_stroke = egui::Stroke::new(1.5_f32, egui::Color32::from_rgb(40, 80, 160));
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
    fonts
        .font_data
        .insert("jp".to_owned(), Arc::new(font_data));
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

#[derive(Default)]
struct RToolsApp {
    log: String,
}

impl RToolsApp {
    fn log_line(&mut self, line: impl AsRef<str>) {
        if !self.log.is_empty() && !self.log.ends_with('\n') {
            self.log.push('\n');
        }
        self.log.push_str(line.as_ref());
        self.log.push('\n');
    }

    fn export_file_list(&mut self) {
        let Some(folder) = rfd::FileDialog::new()
            .set_title("フォルダを選択")
            .pick_folder()
        else {
            self.log_line("フォルダ選択をキャンセルしました。");
            return;
        };

        self.log_line(format!("フォルダ: {}", folder.display()));

        let names = match list_files_in_folder(&folder) {
            Ok(names) => names,
            Err(err) => {
                self.log_line(format!("ファイル一覧の取得に失敗しました: {err}"));
                return;
            }
        };

        self.log_line(format!("ファイル数: {}", names.len()));

        let Some(save_path) = rfd::FileDialog::new()
            .set_title("ファイル一覧を保存")
            .add_filter("テキスト", &["txt"])
            .set_file_name("file_list.txt")
            .save_file()
        else {
            self.log_line("保存をキャンセルしました。");
            return;
        };

        let contents = names.join("\n");
        match fs::write(&save_path, contents) {
            Ok(()) => self.log_line(format!("保存しました: {}", save_path.display())),
            Err(err) => self.log_line(format!("保存に失敗しました: {err}")),
        }
    }
}

impl eframe::App for RToolsApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("toolbar")
            .min_height(56.0)
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(16, 12)),
            )
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    if ui
                        .add(
                            egui::Button::new("フォルダ内のファイル一覧を保存")
                                .min_size(egui::vec2(0.0, 40.0))
                                .corner_radius(6.0)
                                .fill(egui::Color32::from_rgb(232, 240, 254)),
                        )
                        .clicked()
                    {
                        self.export_file_list();
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
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .stick_to_bottom(true)
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.log)
                                    .desired_width(f32::INFINITY)
                                    .frame(false),
                            );
                        });
                });
        });
    }
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
