//! Окно приложения.

use std::time::{Duration, Instant};

use eframe::egui::{self, Color32, RichText, Stroke, Vec2};
use switchdesk_core::settings::{normalize_addr, Peer, Settings};
use switchdesk_core::state::{Listen, Mode, Snapshot, UpdateState};
use switchdesk_core::Engine;

const BG: Color32 = Color32::from_rgb(0x0f, 0x11, 0x17);
const CARD: Color32 = Color32::from_rgb(0x18, 0x1b, 0x23);
const BORDER: Color32 = Color32::from_rgb(0x26, 0x2a, 0x35);
const TEXT: Color32 = Color32::from_rgb(0xe6, 0xe8, 0xee);
const MUTED: Color32 = Color32::from_rgb(0x8b, 0x92, 0xa5);
const ACCENT: Color32 = Color32::from_rgb(0x4c, 0x8d, 0xff);
const GREEN: Color32 = Color32::from_rgb(0x35, 0xc4, 0x7c);
const AMBER: Color32 = Color32::from_rgb(0xf2, 0xa9, 0x3b);
const RED: Color32 = Color32::from_rgb(0xef, 0x5b, 0x5b);

pub const HOTKEY: &str = "Ctrl + Alt + F12";

pub fn apply_theme(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = CARD;
    v.extreme_bg_color = Color32::from_rgb(0x0b, 0x0d, 0x12);
    v.override_text_color = Some(TEXT);
    v.selection.bg_fill = ACCENT;
    v.hyperlink_color = ACCENT;
    let r = egui::CornerRadius::same(8);
    for w in [
        &mut v.widgets.noninteractive,
        &mut v.widgets.inactive,
        &mut v.widgets.hovered,
        &mut v.widgets.active,
        &mut v.widgets.open,
    ] {
        w.corner_radius = r;
    }
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, BORDER);
    v.widgets.inactive.bg_fill = Color32::from_rgb(0x22, 0x26, 0x31);
    v.widgets.inactive.weak_bg_fill = Color32::from_rgb(0x22, 0x26, 0x31);
    v.widgets.hovered.bg_fill = Color32::from_rgb(0x2b, 0x30, 0x3d);
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(0x2b, 0x30, 0x3d);
    // Тема не должна зависеть от системной (иначе виджеты рисуются светлыми на тёмном фоне).
    ctx.set_theme(egui::ThemePreference::Dark);
    ctx.set_visuals_of(egui::Theme::Dark, v);

    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 7.0);
        s.spacing.interact_size.y = 30.0;
        s.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.5));
        s.text_styles.insert(egui::TextStyle::Small, egui::FontId::proportional(12.5));
        s.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.0));
    });
}

pub struct App {
    engine: Engine,
    snap: Snapshot,
    draft: Settings,
    new_name: String,
    new_addr: String,
    add_error: Option<String>,
    copied_at: Option<Instant>,
    demo: Option<Demo>,
}

/// Прогон для снимков окна в CI: по очереди показывает типичные состояния.
struct Demo {
    dir: std::path::PathBuf,
    scene: usize,
    frames: u32,
    requested: bool,
    tx: std::sync::mpsc::Sender<std::sync::Arc<egui::ColorImage>>,
    rx: std::sync::mpsc::Receiver<std::sync::Arc<egui::ColorImage>>,
}

const SCENES: [&str; 5] = ["1-first-run", "2-ready", "3-remote", "4-hosting-update", "5-errors"];

impl App {
    pub fn new(engine: Engine) -> App {
        let draft = engine.settings();
        let snap = engine.snapshot();
        App { engine, snap, draft, new_name: String::new(), new_addr: String::new(), add_error: None, copied_at: None, demo: None }
    }

    pub fn demo(engine: Engine, dir: std::path::PathBuf) -> App {
        let mut app = App::new(engine);
        let _ = std::fs::create_dir_all(&dir);
        let (tx, rx) = std::sync::mpsc::channel();
        app.demo = Some(Demo { dir, scene: 0, frames: 0, requested: false, tx, rx });
        app.apply_scene(0);
        app
    }

    fn apply_scene(&mut self, n: usize) {
        use switchdesk_core::state;
        let peers = vec![
            Peer { name: "Рабочий ПК".into(), addr: "100.101.5.20:7878".into() },
            Peer { name: "Ноутбук".into(), addr: "laptop.tail1234.ts.net:7878".into() },
        ];
        let mut d = Settings { device_name: "Домашний ПК".into(), ..Settings::default() };
        state::set_mode(Mode::Local);
        state::set_hosting(None);
        state::set_listen(Listen::On("100.101.102.103:7878".into()));
        state::set_update(UpdateState::UpToDate);
        match n {
            0 => {}
            1 => {
                d.peers = peers;
                d.target = "Рабочий ПК".into();
            }
            2 => {
                d.peers = peers;
                d.target = "Рабочий ПК".into();
                state::set_mode(Mode::Remote("Рабочий ПК".into()));
                state::log("Подключено: WORK-PC");
                state::log("Ввод переключён на «Рабочий ПК»");
            }
            3 => {
                d.peers = peers;
                d.target = "Ноутбук".into();
                state::set_hosting(Some("Ноутбук".into()));
                state::set_update(UpdateState::Available("0.2.1".into()));
                state::log("«Ноутбук» управляет этим компьютером");
            }
            _ => {
                d.peers = peers;
                d.target = "Ноутбук".into();
                state::set_listen(Listen::Error("Tailscale не найден: запустите его или укажите адрес вручную".into()));
                state::set_update(UpdateState::Failed("не удалось проверить обновления: HTTP 403".into()));
                state::log("Не удалось подключиться к «Ноутбук» (laptop.tail1234.ts.net:7878): connection refused");
            }
        }
        self.draft = d;
    }

    fn demo_step(&mut self, ctx: &egui::Context) {
        let Some(demo) = self.demo.as_mut() else { return };
        ctx.request_repaint();
        if let Ok(img) = demo.rx.try_recv() {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let png = crate::png::encode_rgba(img.size[0] as u32, img.size[1] as u32, &rgba);
            let _ = std::fs::write(demo.dir.join(format!("{}.png", SCENES[demo.scene])), png);
            demo.scene += 1;
            demo.frames = 0;
            demo.requested = false;
            if demo.scene >= SCENES.len() {
                std::process::exit(0);
            }
            let n = demo.scene;
            self.apply_scene(n);
            return;
        }
        demo.frames += 1;
        if demo.frames >= 6 && !demo.requested {
            demo.requested = true;
            let (tx, c) = (demo.tx.clone(), ctx.clone());
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let _ = tx.send(img);
                c.request_repaint();
            })));
        }
    }

    fn dot(ui: &mut egui::Ui, color: Color32) {
        let (rect, _) = ui.allocate_exact_size(Vec2::splat(12.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 5.0, color);
    }

    fn card(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(CARD)
            .stroke(Stroke::new(1.0, BORDER))
            .corner_radius(12)
            .inner_margin(16)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if !title.is_empty() {
                    ui.label(RichText::new(title).size(15.5).strong());
                    ui.add_space(2.0);
                }
                add(ui);
            });
        ui.add_space(12.0);
    }

    fn big_button(ui: &mut egui::Ui, text: &str, fill: Color32, enabled: bool) -> bool {
        let b = egui::Button::new(RichText::new(text).size(16.0).strong().color(Color32::WHITE))
            .fill(fill)
            .corner_radius(10);
        ui.add_enabled(enabled, b.min_size(Vec2::new(ui.available_width(), 46.0))).clicked()
    }

    // ───────────── карточки ─────────────

    fn update_banner(&mut self, ui: &mut egui::Ui) {
        let UpdateState::Available(v) = self.snap.update.clone() else { return };
        let busy = self.snap.mode != Mode::Local || self.snap.hosting.is_some();
        egui::Frame::new()
            .fill(Color32::from_rgb(0x2a, 0x22, 0x10))
            .stroke(Stroke::new(1.0, AMBER))
            .corner_radius(12)
            .inner_margin(12)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.label(RichText::new(format!("Доступна версия {v}")).color(AMBER).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let b = egui::Button::new(RichText::new("Обновить").color(Color32::WHITE)).fill(ACCENT);
                        if ui.add_enabled(!busy, b).on_disabled_hover_text("Сначала верните ввод на этот компьютер").clicked() {
                            self.engine.install_update(true);
                        }
                    });
                });
            });
        ui.add_space(12.0);
    }

    fn status_card(&mut self, ui: &mut egui::Ui) {
        let target = self.draft.target_peer().map(|p| p.name.clone());
        let snap = self.snap.clone();
        Self::card(ui, "", |ui| {
            let (color, title, sub): (Color32, String, String) = match (&snap.mode, &snap.hosting) {
                (Mode::Remote(p), _) => (
                    GREEN,
                    format!("Ввод идёт на «{p}»"),
                    "Клавиатура и мышь сейчас управляют тем компьютером.".into(),
                ),
                (Mode::Connecting(p), _) => (AMBER, format!("Подключаюсь к «{p}»…"), "Подождите пару секунд.".into()),
                (Mode::Local, Some(who)) => (
                    AMBER,
                    format!("«{who}» управляет этим компьютером"),
                    "Если начать работать за ним, чужой ввод временно игнорируется.".into(),
                ),
                (Mode::Local, None) if target.is_some() => {
                    (ACCENT, "Ввод на этом компьютере".into(), "Нажмите кнопку или горячую клавишу, чтобы переключиться.".into())
                }
                (Mode::Local, None) => (
                    MUTED,
                    "Добавьте компьютер".into(),
                    "Укажите ниже, на какой компьютер переключать ввод.".into(),
                ),
            };
            ui.horizontal(|ui| {
                Self::dot(ui, color);
                ui.label(RichText::new(title).size(18.0).strong());
            });
            ui.label(RichText::new(sub).color(MUTED));
            ui.add_space(6.0);
            match (&snap.mode, &target) {
                (Mode::Remote(_), _) => {
                    if Self::big_button(ui, "Вернуться на этот компьютер", Color32::from_rgb(0x3a, 0x40, 0x52), true) {
                        self.engine.toggle();
                    }
                }
                (Mode::Connecting(_), _) => {
                    Self::big_button(ui, "Подключаюсь…", Color32::from_rgb(0x3a, 0x40, 0x52), false);
                }
                (Mode::Local, Some(t)) => {
                    let can = snap.hosting.is_none();
                    let label = if can { format!("Переключить на «{t}»") } else { "Недоступно, пока вами управляют".to_string() };
                    if Self::big_button(ui, &label, if can { ACCENT } else { Color32::from_rgb(0x3a, 0x40, 0x52) }, can) {
                        self.engine.toggle();
                    }
                }
                (Mode::Local, None) => {}
            }
            ui.add_space(2.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Горячая клавиша").color(MUTED));
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x22, 0x26, 0x31))
                    .stroke(Stroke::new(1.0, BORDER))
                    .corner_radius(6)
                    .inner_margin(egui::Margin::symmetric(8, 3))
                    .show(ui, |ui| ui.label(RichText::new(HOTKEY).monospace()));
            });
        });
    }

    fn peers_card(&mut self, ui: &mut egui::Ui, dirty: &mut bool) {
        Self::card(ui, "Компьютеры, на которые можно переключаться", |ui| {
            if self.draft.peers.is_empty() {
                ui.label(
                    RichText::new(
                        "Пока пусто. Установите Switchdesk на втором компьютере, откройте его — \
                         там будет показан адрес для подключения. Введите его здесь.",
                    )
                    .color(MUTED),
                );
            }
            let selected = self.draft.target_peer().map(|p| p.name.clone());
            let mut remove: Option<usize> = None;
            for (i, p) in self.draft.peers.iter().enumerate() {
                ui.horizontal(|ui| {
                    let is_sel = selected.as_deref() == Some(p.name.as_str());
                    if ui.radio(is_sel, "").clicked() && !is_sel {
                        self.draft.target = p.name.clone();
                        *dirty = true;
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new(&p.name).strong());
                        ui.label(RichText::new(&p.addr).small().color(MUTED));
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Удалить").clicked() {
                            remove = Some(i);
                        }
                    });
                });
            }
            if let Some(i) = remove {
                let gone = self.draft.peers.remove(i);
                if self.draft.target == gone.name {
                    self.draft.target = self.draft.peers.first().map(|p| p.name.clone()).unwrap_or_default();
                }
                *dirty = true;
            }

            ui.add_space(6.0);
            ui.separator();
            ui.label(RichText::new("Добавить компьютер").color(MUTED).small());
            ui.add(
                egui::TextEdit::singleline(&mut self.new_name)
                    .hint_text("Название, например «Рабочий ПК»")
                    .desired_width(f32::INFINITY),
            );
            ui.add(
                egui::TextEdit::singleline(&mut self.new_addr)
                    .hint_text("Адрес: 100.x.y.z или имя в Tailscale")
                    .desired_width(f32::INFINITY),
            );
            if let Some(e) = &self.add_error {
                ui.label(RichText::new(e).color(RED).small());
            }
            if ui.button("Добавить").clicked() {
                let name = self.new_name.trim().to_owned();
                match (name.is_empty(), normalize_addr(&self.new_addr)) {
                    (true, _) => self.add_error = Some("Введите название".into()),
                    (_, None) => self.add_error = Some("Адрес указан неверно".into()),
                    _ if self.draft.peers.iter().any(|p| p.name == name) => {
                        self.add_error = Some("Компьютер с таким названием уже есть".into())
                    }
                    (_, Some(addr)) => {
                        self.draft.peers.push(Peer { name: name.clone(), addr });
                        if self.draft.target.is_empty() {
                            self.draft.target = name;
                        }
                        self.new_name.clear();
                        self.new_addr.clear();
                        self.add_error = None;
                        *dirty = true;
                    }
                }
            }
        });
    }

    fn this_pc_card(&mut self, ui: &mut egui::Ui, dirty: &mut bool) {
        let listen = self.snap.listen.clone();
        Self::card(ui, "Этот компьютер", |ui| {
            ui.label(RichText::new("Название").color(MUTED).small());
            let r = ui.add(egui::TextEdit::singleline(&mut self.draft.device_name).desired_width(f32::INFINITY));
            if r.lost_focus() {
                *dirty = true;
            }
            ui.add_space(4.0);
            if ui.checkbox(&mut self.draft.accept_input, "Принимать ввод с других компьютеров").changed() {
                *dirty = true;
            }
            match &listen {
                Listen::On(addr) => {
                    ui.horizontal(|ui| {
                        Self::dot(ui, GREEN);
                        ui.label("Готов принимать. Адрес для подключения:");
                    });
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(addr).monospace().size(15.0).strong());
                        let copied = self.copied_at.is_some_and(|t| t.elapsed() < Duration::from_secs(2));
                        if ui.button(if copied { "Скопировано" } else { "Копировать" }).clicked() {
                            ui.ctx().copy_text(addr.clone());
                            self.copied_at = Some(Instant::now());
                        }
                    });
                }
                Listen::Error(e) => {
                    ui.horizontal_wrapped(|ui| {
                        Self::dot(ui, RED);
                        ui.label(RichText::new(e).color(RED));
                    });
                }
                Listen::Off => {
                    ui.horizontal(|ui| {
                        Self::dot(ui, MUTED);
                        ui.label(RichText::new("Приём ввода выключен").color(MUTED));
                    });
                }
            }
            ui.add_space(4.0);
            ui.collapsing("Дополнительно", |ui| {
                ui.label(RichText::new("Адрес приёма (auto — адрес Tailscale)").color(MUTED).small());
                let r = ui.add(egui::TextEdit::singleline(&mut self.draft.listen).desired_width(f32::INFINITY));
                if r.lost_focus() {
                    *dirty = true;
                }
                ui.horizontal(|ui| {
                    ui.label("Порт");
                    if ui.add(egui::DragValue::new(&mut self.draft.port).range(1..=65535)).changed() {
                        *dirty = true;
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Пауза после вашего ввода, мс");
                    if ui.add(egui::DragValue::new(&mut self.draft.grace_ms).range(0..=10_000).speed(50)).changed() {
                        *dirty = true;
                    }
                });
            });
        });
    }

    fn app_card(&mut self, ui: &mut egui::Ui, dirty: &mut bool) {
        let update = self.snap.update.clone();
        let busy = self.snap.mode != Mode::Local || self.snap.hosting.is_some();
        Self::card(ui, "Программа", |ui| {
            if ui.checkbox(&mut self.draft.autostart, "Запускать при входе в Windows").changed() {
                *dirty = true;
            }
            if ui.checkbox(&mut self.draft.auto_update, "Обновляться автоматически").changed() {
                *dirty = true;
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("Версия {}", switchdesk_update::CURRENT)).color(MUTED));
                match &update {
                    UpdateState::Checking => {
                        ui.spinner();
                        ui.label("Проверяю…");
                    }
                    UpdateState::Installing => {
                        ui.spinner();
                        ui.label("Устанавливаю…");
                    }
                    UpdateState::UpToDate => {
                        ui.label(RichText::new("Установлена последняя").color(GREEN));
                    }
                    UpdateState::Failed(e) => {
                        ui.label(RichText::new(format!("Ошибка: {e}")).color(RED).small());
                    }
                    _ => {}
                }
            });
            match &update {
                UpdateState::Available(v) => {
                    ui.label(RichText::new(format!("Доступна версия {v}")).color(AMBER).strong());
                    let b = egui::Button::new(RichText::new("Обновить и перезапустить").color(Color32::WHITE)).fill(ACCENT);
                    if ui.add_enabled(!busy, b).on_disabled_hover_text("Сначала верните ввод на этот компьютер").clicked() {
                        self.engine.install_update(true);
                    }
                }
                UpdateState::Checking | UpdateState::Installing => {}
                _ => {
                    if ui.button("Проверить обновления").clicked() {
                        self.engine.check_update();
                    }
                }
            }
        });
    }

    fn log_card(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new(RichText::new("Журнал").color(MUTED)).show(ui, |ui| {
            egui::ScrollArea::vertical().max_height(180.0).stick_to_bottom(true).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for line in &self.snap.logs {
                    ui.label(RichText::new(line).monospace().small().color(MUTED));
                }
            });
        });
    }
}

impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.snap = self.engine.snapshot();
        // Окно скрыто в трее, а пользователь нажал «Закрыть» уже после — ничего не делаем.
        ctx.request_repaint_after(Duration::from_millis(700));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // Крестик прячет окно в трей; выйти можно из меню иконки.
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::CancelClose);
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Visible(false));
        }
        self.demo_step(&ui.ctx().clone());
        self.snap = self.engine.snapshot();
        let mut dirty = false;

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(16))
            .show(ui, |ui| {
                ui.label(RichText::new("Switchdesk").size(22.0).strong());
                ui.add_space(8.0);
                egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                    self.update_banner(ui);
                    self.status_card(ui);
                    self.peers_card(ui, &mut dirty);
                    self.this_pc_card(ui, &mut dirty);
                    self.app_card(ui, &mut dirty);
                    self.log_card(ui);
                });
            });

        if dirty {
            if let Err(e) = self.engine.apply(self.draft.clone()) {
                switchdesk_core::state::log(e);
            }
            // движок мог поправить значения (пустое имя и т. п.)
            self.draft = self.engine.settings();
        }
    }
}
