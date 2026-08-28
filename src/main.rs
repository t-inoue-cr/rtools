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
            Ok(Box::new(RToolsApp::default()))
        }),
    )
}

fn windows_fonts_dir() -> PathBuf {
    let windir = std::env::var_os("WINDIR").unwrap_or_else(|| r"C:\Windows".into());
    PathBuf::from(windir).join("Fonts")
}

fn load_japanese_font() -> Option<egui::FontData> {
    const CANDIDATES: &[&str] = &["YuGothR.ttc", "meiryo.ttc", "msgothic.ttc"];
    let fonts_dir = windows_fonts_dir();
    for name in CANDIDATES {
        if let Ok(bytes) = fs::read(fonts_dir.join(name)) {
            return Some(egui::FontData::from_owned(bytes));
        }
    }
    None
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
        .push("jp".to_owned());
    fonts
        .families
        .entry(egui::FontFamily::Monospace)
        .or_default()
        .push("jp".to_owned());
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
        egui::TopBottomPanel::top("buttons").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if ui.button("フォルダ内のファイル一覧を保存").clicked() {
                    self.export_file_list();
                }
            });
            ui.add_space(6.0);
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label("ログ");
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    ui.add_sized(
                        ui.available_size(),
                        egui::TextEdit::multiline(&mut self.log).desired_width(f32::INFINITY),
                    );
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
