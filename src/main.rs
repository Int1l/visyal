#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod launch;

use eframe::egui::{self, Color32, Frame, Margin, RichText, Rounding, Sense, Stroke, Vec2};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Serialize, Deserialize, Clone)]
pub struct Config {
    pub nickname: String,
    pub version: String,
    pub ram_mb: u32,
    pub java_path: String,
    pub game_dir: String,
    #[serde(default = "yes")]
    pub prefer_opengl: bool,
}

fn yes() -> bool {
    true
}

impl Default for Config {
    fn default() -> Self {
        let game_dir = dirs::data_dir()
            .unwrap_or_default()
            .join(".minecraft")
            .to_string_lossy()
            .to_string();
        Self {
            nickname: "Player".into(),
            version: String::new(),
            ram_mb: 4096,
            java_path: if cfg!(windows) { "javaw".into() } else { "java".into() },
            game_dir,
            prefer_opengl: true,
        }
    }
}

fn config_path() -> PathBuf {
    dirs::config_dir().unwrap_or_default().join("BlackLauncher").join("config.json")
}

fn load_config() -> Config {
    std::fs::read_to_string(config_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_config(c: &Config) {
    let p = config_path();
    if let Some(d) = p.parent() { let _ = std::fs::create_dir_all(d); }
    if let Ok(s) = serde_json::to_string_pretty(c) { let _ = std::fs::write(p, s); }
}

#[derive(PartialEq)]
enum Tab { Home, Settings }

struct App {
    cfg: Config,
    versions: Vec<String>,
    tab: Tab,
    status: Arc<Mutex<(String, bool)>>,
}

const BG: Color32 = Color32::from_rgb(10, 10, 10);
const BORDER: Color32 = Color32::from_rgb(38, 38, 38);
const MUTED: Color32 = Color32::from_rgb(130, 130, 130);

impl App {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let mut v = egui::Visuals::dark();
        v.panel_fill = Color32::TRANSPARENT;
        v.window_fill = BG;
        v.override_text_color = Some(Color32::from_gray(225));
        let rounding = Rounding::same(8.0);
        for (w, fill) in [
            (&mut v.widgets.inactive, 20u8),
            (&mut v.widgets.hovered, 36),
            (&mut v.widgets.active, 52),
        ] {
            w.bg_fill = Color32::from_gray(fill);
            w.weak_bg_fill = Color32::from_gray(fill);
            w.rounding = rounding;
            w.bg_stroke = Stroke::new(1.0, BORDER);
        }
        v.widgets.noninteractive.rounding = rounding;
        v.selection.bg_fill = Color32::from_gray(70);
        cc.egui_ctx.set_visuals(v);

        let cfg = load_config();
        let versions = launch::list_versions(&cfg.game_dir);
        let mut cfg = cfg;
        if cfg.version.is_empty() || !versions.contains(&cfg.version) {
            cfg.version = versions.first().cloned().unwrap_or_default();
        }
        Self { cfg, versions, tab: Tab::Home, status: Arc::new(Mutex::new((String::new(), false))) }
    }

    fn busy(&self) -> bool { self.status.lock().map(|s| s.1).unwrap_or(false) }

    fn play(&mut self) {
        save_config(&self.cfg);
        *self.status.lock().unwrap() = ("Запуск...".into(), true);
        let status = self.status.clone();
        let cfg = self.cfg.clone();
        std::thread::spawn(move || {
            let set = |t: String, b: bool| *status.lock().unwrap() = (t, b);
            match launch::launch(&cfg) {
                Ok(mut c) => {
                    set("Игра запущена".into(), true);
                    match c.wait() {
                        Ok(st) if st.success() => set("Игра закрыта".into(), false),
                        other => {
                            let code = other.ok().and_then(|x| x.code());
                            set(format!("Игра упала (код {:?}). Лог:\n{}", code, launch::log_tail(15)), false);
                        }
                    }
                }
                Err(e) => set(format!("Ошибка: {e}"), false),
            }
        });
    }
}

impl eframe::App for App {
    fn clear_color(&self, _: &egui::Visuals) -> [f32; 4] { [0.0; 4] }

    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if self.busy() { ctx.request_repaint_after(Duration::from_millis(300)); }

        // скруглённый фон окна
        let rect = ctx.screen_rect().shrink(1.0);
        let p = ctx.layer_painter(egui::LayerId::background());
        p.rect_filled(rect, Rounding::same(14.0), BG);
        p.rect_stroke(rect, Rounding::same(14.0), Stroke::new(1.0, BORDER));

        // верхняя панель
        egui::TopBottomPanel::top("top")
            .exact_height(46.0)
            .frame(Frame::none().inner_margin(Margin::symmetric(18.0, 10.0)))
            .show(ctx, |ui| {
                let drag = ui.interact(ui.max_rect(), egui::Id::new("drag"), Sense::click_and_drag());
                if drag.drag_started() { ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag); }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("BLACK").strong().size(16.0));
                    ui.label(RichText::new("v0.1.0").size(12.0).color(MUTED));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(egui::Button::new("x").frame(false)).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                        if ui.add(egui::Button::new("-").frame(false)).clicked() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                        }
                        ui.add_space(10.0);
                        let name = self.cfg.nickname.clone();
                        if ui.add(egui::Button::new(RichText::new(format!("●  {name}")).size(13.0))).clicked() {
                            self.tab = Tab::Settings;
                        }
                    });
                });
            });

        // меню слева
        egui::SidePanel::left("nav")
            .exact_width(150.0)
            .resizable(false)
            .frame(Frame::none().inner_margin(Margin::symmetric(16.0, 8.0)))
            .show(ctx, |ui| {
                ui.add_space(8.0);
                for (t, name) in [(Tab::Home, "Главная"), (Tab::Settings, "Настройки")] {
                    let sel = self.tab == t;
                    let txt = RichText::new(name).size(14.0).color(if sel { Color32::WHITE } else { MUTED });
                    if ui.add_sized([118.0, 34.0], egui::SelectableLabel::new(sel, txt)).clicked() {
                        self.tab = t;
                    }
                }
            });

        egui::CentralPanel::default()
            .frame(Frame::none().inner_margin(Margin::same(24.0)))
            .show(ctx, |ui| match self.tab {
                Tab::Home => {
                    ui.vertical_centered(|ui| {
                        ui.add_space(70.0);
                        ui.label(RichText::new("BLACK").size(44.0).strong());
                        ui.label(RichText::new("минималистичный клиент").color(MUTED));
                        ui.add_space(36.0);
                        egui::ComboBox::from_id_salt("ver")
                            .width(220.0)
                            .selected_text(if self.cfg.version.is_empty() { "нет версий".into() } else { self.cfg.version.clone() })
                            .show_ui(ui, |ui| {
                                for v in self.versions.clone() {
                                    ui.selectable_value(&mut self.cfg.version, v.clone(), v);
                                }
                            });
                        ui.add_space(18.0);
                        let busy = self.busy();
                        let btn = egui::Button::new(RichText::new("ИГРАТЬ").size(18.0).strong().color(Color32::BLACK))
                            .fill(Color32::WHITE)
                            .rounding(10.0)
                            .min_size(Vec2::new(220.0, 52.0));
                        if ui.add_enabled(!busy && !self.cfg.version.is_empty(), btn).clicked() {
                            self.play();
                        }
                        ui.add_space(14.0);
                        let s = self.status.lock().unwrap().0.clone();
                        egui::ScrollArea::vertical().max_height(170.0).show(ui, |ui| {
                            ui.label(RichText::new(s).size(11.0).color(MUTED));
                        });
                        if ui.button("Открыть лог").clicked() {
                            let _ = std::process::Command::new(if cfg!(windows) { "notepad" } else { "xdg-open" })
                                .arg(launch::log_path())
                                .spawn();
                        }
                    });
                }
                Tab::Settings => {
                    ui.label(RichText::new("Настройки").size(22.0).strong());
                    ui.add_space(16.0);
                    ui.label("Ник (офлайн аккаунт)");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.nickname).desired_width(300.0));
                    ui.add_space(10.0);
                    ui.label("Оперативная память (МБ)");
                    ui.add(egui::Slider::new(&mut self.cfg.ram_mb, 1024..=16384).step_by(512.0));
                    ui.add_space(10.0);
                    ui.label("Java (javaw или полный путь)");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.java_path).desired_width(420.0));
                    ui.add_space(10.0);
                    ui.label("Папка игры (.minecraft)");
                    ui.add(egui::TextEdit::singleline(&mut self.cfg.game_dir).desired_width(420.0));
                    ui.add_space(12.0);
                    ui.checkbox(&mut self.cfg.prefer_opengl, "Использовать OpenGL вместо Vulkan (для старых видеокарт)");
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        if ui.button("Сохранить").clicked() { save_config(&self.cfg); }
                        if ui.button("Обновить версии").clicked() {
                            self.versions = launch::list_versions(&self.cfg.game_dir);
                            if !self.versions.contains(&self.cfg.version) {
                                self.cfg.version = self.versions.first().cloned().unwrap_or_default();
                            }
                        }
                    });
                }
            });
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([920.0, 580.0])
            .with_min_inner_size([720.0, 460.0])
            .with_decorations(false)
            .with_transparent(true),
        ..Default::default()
    };
    eframe::run_native("Black Launcher", options, Box::new(|cc| Ok(Box::new(App::new(cc)))))
}
