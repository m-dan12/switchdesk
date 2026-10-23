use crate::capture;
use crate::client::{Client, Status};
use crate::config::{self, Config, Device};
use crate::host::{Host, HostShared};
use crate::updater::{UpdState, Updater};
use crate::winapp;
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, Theme,
    ThemePreference, Ui,
};
use std::sync::atomic::Ordering::SeqCst;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tray_icon::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};

const VERSION: &str = switchdesk_update::CURRENT;
const UPDATE_EVERY: Duration = Duration::from_secs(24 * 3600);

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Devices,
    Host,
    Settings,
}

#[derive(Clone, Copy)]
struct Pal {
    bg: Color32,
    side: Color32,
    card: Color32,
    bd: Color32,
    tx: Color32,
    tx2: Color32,
    ac: Color32,
    acbg: Color32,
    ok: Color32,
    okbg: Color32,
    warn: Color32,
    hov: Color32,
}

fn c(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn pal(dark: bool) -> Pal {
    if dark {
        Pal {
            bg: c(0x1c1c1a),
            side: c(0x232321),
            card: c(0x2a2a27),
            bd: c(0x3a3a36),
            tx: c(0xecebe6),
            tx2: c(0x9c9b94),
            ac: c(0x6ea0ff),
            acbg: c(0x26344f),
            ok: c(0x5fc78c),
            okbg: c(0x223a2d),
            warn: c(0xe3b04f),
            hov: c(0x30302c),
        }
    } else {
        Pal {
            bg: c(0xf6f6f4),
            side: c(0xefeeea),
            card: c(0xffffff),
            bd: c(0xe2e0d9),
            tx: c(0x1d1d1b),
            tx2: c(0x6b6a64),
            ac: c(0x2f6fe4),
            acbg: c(0xe6eefc),
            ok: c(0x2e8b57),
            okbg: c(0xe3f3ea),
            warn: c(0xa8680b),
            hov: c(0xe7e5df),
        }
    }
}

fn apply_theme(ctx: &egui::Context, p: Pal, dark: bool) {
    let mut v = if dark { egui::Visuals::dark() } else { egui::Visuals::light() };
    v.panel_fill = p.bg;
    v.window_fill = p.card;
    v.extreme_bg_color = p.card;
    v.faint_bg_color = p.side;
    v.selection.bg_fill = p.acbg;
    v.selection.stroke = Stroke::new(1.0, p.ac);
    v.hyperlink_color = p.ac;
    let r = CornerRadius::same(8);
    let w = &mut v.widgets;
    w.noninteractive.bg_fill = p.bg;
    w.noninteractive.bg_stroke = Stroke::new(1.0, p.bd);
    w.noninteractive.fg_stroke = Stroke::new(1.0, p.tx);
    for (s, fill) in [(&mut w.inactive, p.card), (&mut w.hovered, p.hov), (&mut w.active, p.hov), (&mut w.open, p.hov)] {
        s.bg_fill = fill;
        s.weak_bg_fill = fill;
        s.bg_stroke = Stroke::new(1.0, p.bd);
        s.fg_stroke = Stroke::new(1.0, p.tx);
        s.corner_radius = r;
    }
    w.noninteractive.corner_radius = r;
    w.hovered.bg_stroke = Stroke::new(1.0, p.tx2);
    ctx.set_visuals_of(if dark { Theme::Dark } else { Theme::Light }, v);
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 8.0);
        s.spacing.button_padding = vec2(14.0, 8.0);
        s.spacing.interact_size.y = 32.0;
    });
}

fn text(s: impl Into<String>, color: Color32) -> RichText {
    RichText::new(s.into()).color(color)
}

fn card<R>(ui: &mut Ui, p: Pal, border: Color32, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(p.card)
        .stroke(Stroke::new(1.0, border))
        .corner_radius(10)
        .inner_margin(Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

fn pill(ui: &mut Ui, label: &str, fg: Color32, bg: Color32) {
    egui::Frame::new()
        .fill(bg)
        .corner_radius(99)
        .inner_margin(Margin::symmetric(10, 3))
        .show(ui, |ui| ui.label(RichText::new(label).color(fg).size(12.0)));
}

fn toggle(ui: &mut Ui, on: &mut bool, p: Pal) -> bool {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(36.0, 20.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let how = ui.ctx().animate_bool_responsive(resp.id, *on);
    let bg = p.bd.lerp_to_gamma(p.ac, how);
    ui.painter().rect_filled(rect, 10, bg);
    let x = egui::lerp(rect.left() + 10.0..=rect.right() - 10.0, how);
    ui.painter().circle_filled(egui::pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp.changed()
}

fn row_setting(ui: &mut Ui, p: Pal, title: &str, hint: &str, right: impl FnOnce(&mut Ui)) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.label(text(title, p.tx));
            if !hint.is_empty() {
                ui.label(text(hint, p.tx2).size(12.0));
            }
        });
        ui.with_layout(Layout::right_to_left(Align::Center), right);
    });
}

fn divider(ui: &mut Ui, p: Pal) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().rect_filled(rect, 0, p.bd);
}

fn primary(p: Pal, label: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(label).color(Color32::WHITE).strong())
        .fill(p.ac)
        .stroke(Stroke::new(1.0, p.ac))
}

pub struct App {
    cfg: Config,
    tab: Tab,
    selected: usize,
    editing: bool,
    client: Client,
    host: Option<Host>,
    host_shared: Arc<HostShared>,
    host_error: Option<String>,
    addrs: Vec<String>,
    updater: Updater,
    last_update_check: Instant,
    autostart: bool,
    start_hidden: bool,
    _tray: Option<TrayIcon>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, minimized: bool, tab: Option<&str>) -> App {
        let cfg = Config::load();
        let client = Client::new();
        let updater = Updater::new();
        crate::control::listen(client.clone());
        let tray = make_tray(&client, &updater);
        cc.egui_ctx.options_mut(|o| o.theme_preference = theme_pref(&cfg.theme));
        let mut app = App {
            selected: cfg.last_device,
            editing: false,
            host_shared: HostShared::new(cfg.host_allow_input),
            tab: match tab {
                Some("host") => Tab::Host,
                Some("settings") => Tab::Settings,
                _ => Tab::Devices,
            },
            client,
            host: None,
            host_error: None,
            addrs: local_addrs(),
            last_update_check: Instant::now(),
            autostart: winapp::autostart_enabled(),
            start_hidden: minimized && tray.is_some() && cfg.close_to_tray,
            _tray: tray,
            updater,
            cfg,
        };
        app.updater.check();
        if app.cfg.host_enabled {
            app.start_host();
        }
        app
    }

    fn start_host(&mut self) {
        match Host::start(
            self.cfg.host_port,
            self.cfg.host_token.clone(),
            self.cfg.pc_name.clone(),
            self.host_shared.clone(),
        ) {
            Ok(h) => {
                self.host = Some(h);
                self.host_error = None;
            }
            Err(e) => {
                self.host_error = Some(format!("Не удалось открыть порт {}: {e}", self.cfg.host_port));
                self.cfg.host_enabled = false;
            }
        }
    }

    fn stop_host(&mut self) {
        self.host = None; // Drop закрывает все соединения
        self.host_shared.clients.lock().unwrap().clear();
    }

    // ----- шапка -----

    fn update_banner(&mut self, ui: &mut Ui, p: Pal) {
        let st = self.updater.state();
        let (msg, button, error) = match &st {
            UpdState::Available(r) => (format!("Доступна версия {} (у вас {VERSION})", r.version), Some("Обновить и перезапустить"), false),
            UpdState::Downloading(r) => (format!("Скачиваю версию {}…", r.version), None, false),
            UpdState::Error(e) if self.last_update_check.elapsed() < Duration::from_secs(1) || e.contains("подпис") => {
                (format!("Обновление не удалось: {e}"), None, true)
            }
            _ => return,
        };
        egui::Frame::new().fill(if error { p.bd } else { p.acbg }).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(text(msg, if error { p.warn } else { p.ac }).size(13.0));
                if let Some(b) = button {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(primary(p, b)).clicked() {
                            self.updater.install();
                        }
                    });
                }
            });
        });
    }

    fn header(&mut self, ui: &mut Ui, p: Pal) {
        egui::Frame::new().fill(p.side).inner_margin(Margin { left: 14, right: 14, top: 10, bottom: 0 }).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(text("SwitchDesk", p.tx).strong().size(15.0));
                ui.label(text(format!("v{VERSION}"), p.tx2).size(12.0));
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                for (tab, label) in [(Tab::Devices, "Устройства"), (Tab::Host, "Этот ПК"), (Tab::Settings, "Настройки")] {
                    let on = self.tab == tab;
                    let resp = ui.add(
                        egui::Button::new(text(label, if on { p.tx } else { p.tx2 }))
                            .fill(if on { p.bg } else { Color32::TRANSPARENT })
                            .stroke(Stroke::new(1.0, if on { p.bd } else { Color32::TRANSPARENT }))
                            .corner_radius(CornerRadius { nw: 8, ne: 8, sw: 0, se: 0 }),
                    );
                    if resp.clicked() {
                        self.tab = tab;
                    }
                }
            });
        });
    }

    // ----- вкладка «Устройства» -----

    fn devices_tab(&mut self, ui: &mut Ui, p: Pal) {
        let status = self.client.status();
        egui::Panel::left("devices")
            .exact_size(220.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(p.side).inner_margin(10))
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for i in 0..self.cfg.devices.len() {
                    let connected_here = i == self.selected && matches!(status, Status::Connected(_));
                    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
                    let sel = i == self.selected;
                    if sel || resp.hovered() {
                        ui.painter().rect_filled(rect, 8, if sel { p.acbg } else { p.hov });
                    }
                    let dot = if connected_here { p.ok } else { p.tx2.gamma_multiply(0.5) };
                    ui.painter().circle_filled(rect.left_center() + vec2(14.0, 0.0), 4.0, dot);
                    let d = &self.cfg.devices[i];
                    let name = if d.name.is_empty() { "(без имени)" } else { &d.name };
                    ui.painter().text(rect.left_top() + vec2(30.0, 9.0), Align2::LEFT_TOP, name, FontId::proportional(14.0), p.tx);
                    ui.painter().text(
                        rect.left_top() + vec2(30.0, 27.0),
                        Align2::LEFT_TOP,
                        if d.addr.is_empty() { "адрес не задан" } else { &d.addr },
                        FontId::proportional(11.0),
                        p.tx2,
                    );
                    if resp.clicked() {
                        self.selected = i;
                        self.editing = false;
                    }
                }
                ui.add_space(6.0);
                let add = ui.add_sized(
                    [ui.available_width(), 34.0],
                    egui::Button::new(text("+ Добавить устройство", p.tx2)).fill(Color32::TRANSPARENT),
                );
                if add.clicked() {
                    self.cfg.devices.push(Device { name: format!("Компьютер {}", self.cfg.devices.len() + 1), ..Default::default() });
                    self.selected = self.cfg.devices.len() - 1;
                    self.editing = true;
                    self.cfg.save();
                }
            });

        egui::Frame::new().inner_margin(16).show(ui, |ui| {
            if self.selected >= self.cfg.devices.len() {
                ui.add_space(30.0);
                ui.vertical_centered(|ui| {
                    ui.label(text("Добавьте первое устройство", p.tx).size(16.0).strong());
                    ui.label(text("Адрес и токен покажет вкладка «Этот ПК» на целевом компьютере.", p.tx2));
                });
                return;
            }
            let i = self.selected;
            let (dname, daddr) = (self.cfg.devices[i].name.clone(), self.cfg.devices[i].addr.clone());
            card(ui, p, p.bd, |ui| {
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(text(&dname, p.tx).size(16.0).strong());
                        ui.label(text(if daddr.is_empty() { "адрес не задан" } else { &daddr }, p.tx2));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| match &status {
                        Status::Connected(_) => pill(ui, "Подключено", p.ok, p.okbg),
                        Status::Connecting => pill(ui, "Подключение…", p.tx2, p.hov),
                        Status::Error(_) => pill(ui, "Ошибка", p.warn, p.hov),
                        Status::Idle => pill(ui, "Не подключено", p.tx2, p.hov),
                    });
                });
                if let Status::Error(e) = &status {
                    ui.label(text(e, p.warn).size(12.0));
                }
            });

            if self.editing {
                card(ui, p, p.bd, |ui| {
                    egui::Grid::new("dev").num_columns(2).spacing([12.0, 10.0]).show(ui, |ui| {
                        let d = &mut self.cfg.devices[i];
                        ui.label(text("Название", p.tx2));
                        ui.add(egui::TextEdit::singleline(&mut d.name).desired_width(f32::INFINITY));
                        ui.end_row();
                        ui.label(text("Адрес", p.tx2));
                        ui.add(
                            egui::TextEdit::singleline(&mut d.addr)
                                .hint_text("100.x.y.z или 192.168.1.10:52800")
                                .desired_width(f32::INFINITY),
                        );
                        ui.end_row();
                        ui.label(text("Токен", p.tx2));
                        ui.add(egui::TextEdit::singleline(&mut d.token).password(true).desired_width(f32::INFINITY));
                        ui.end_row();
                    });
                    ui.horizontal(|ui| {
                        if ui.button("Готово").clicked() {
                            self.editing = false;
                            self.cfg.save();
                        }
                        if ui.button(text("Удалить устройство", p.warn)).clicked() {
                            self.client.disconnect();
                            self.cfg.devices.remove(i);
                            self.selected = self.selected.saturating_sub(1);
                            self.editing = false;
                            self.cfg.save();
                        }
                    });
                });
            }

            let connected = matches!(status, Status::Connected(_));
            if connected {
                let active = capture::is_active();
                let border = if active { p.warn } else { p.bd };
                card(ui, p, border, |ui| {
                    ui.horizontal(|ui| {
                        ui.vertical(|ui| {
                            if active {
                                ui.label(text(format!("Управление передано на {dname}"), p.warn).strong());
                                ui.label(text("Мышь и клавиатура работают на удалённом ПК", p.tx2).size(12.0));
                            } else {
                                ui.label(text("Управление у вас", p.tx).strong());
                                ui.label(text("Нажмите кнопку или горячую клавишу, чтобы передать ввод", p.tx2).size(12.0));
                            }
                        });
                        if active {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.button("Вернуть себе").clicked() {
                                    capture::toggle();
                                }
                            });
                        }
                    });
                });
                ui.label(text("Ctrl + Alt + F12 включает и выключает передачу из любого окна. Текстовый буфер обмена синхронизируется.", p.tx2).size(12.0));
            }

            ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
                ui.horizontal(|ui| {
                    match &status {
                        Status::Connected(_) | Status::Connecting => {
                            if ui.button("Отключиться").clicked() {
                                self.client.disconnect();
                            }
                        }
                        _ => {
                            if ui.add(primary(p, "Подключиться")).clicked() {
                                self.cfg.last_device = i;
                                self.cfg.save();
                                self.client.connect(self.cfg.devices[i].clone(), self.cfg.pc_name.clone());
                            }
                        }
                    }
                    if !self.editing && ui.button("Изменить").clicked() {
                        self.editing = true;
                    }
                    if connected {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if capture::is_active() { "Вернуть управление себе" } else { "Управлять этим компьютером" };
                            if ui.add(primary(p, label)).clicked() {
                                capture::toggle();
                            }
                        });
                    }
                });
            });
        });
    }

    // ----- вкладка «Этот ПК» -----

    fn host_tab(&mut self, ui: &mut Ui, p: Pal) {
        egui::Frame::new().inner_margin(16).show(ui, |ui| {
            let mut changed = false;
            let running = self.host.is_some();
            card(ui, p, p.bd, |ui| {
                let mut enabled = running;
                let addrs = self.addrs.join(", ");
                row_setting(
                    ui,
                    p,
                    "Принимать подключения",
                    &format!("Порт {} · адреса: {}", self.cfg.host_port, if addrs.is_empty() { "—".into() } else { addrs }),
                    |ui| {
                        if toggle(ui, &mut enabled, p) {
                            if enabled {
                                self.start_host();
                            } else {
                                self.stop_host();
                            }
                            self.cfg.host_enabled = self.host.is_some();
                            changed = true;
                        }
                    },
                );
                if let Some(e) = &self.host_error {
                    ui.label(text(e, p.warn).size(12.0));
                }
                divider(ui, p);
                let mut allow = self.host_shared.allow_input.load(SeqCst);
                row_setting(ui, p, "Разрешить управление", "Выключите, чтобы мгновенно заморозить ввод", |ui| {
                    if toggle(ui, &mut allow, p) {
                        self.host_shared.allow_input.store(allow, SeqCst);
                        self.cfg.host_allow_input = allow;
                        changed = true;
                    }
                });
                divider(ui, p);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.label(text("Токен", p.tx));
                        ui.label(text(&self.cfg.host_token, p.tx2).monospace().size(12.0));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add_enabled(!running, egui::Button::new("Новый")).clicked() {
                            self.cfg.host_token = config::new_token();
                            changed = true;
                        }
                        if ui.button("Копировать").clicked() {
                            ui.ctx().copy_text(self.cfg.host_token.clone());
                        }
                    });
                });
                divider(ui, p);
                row_setting(ui, p, "Имя этого ПК", "Видно клиентам при подключении", |ui| {
                    changed |= ui.add_enabled(!running, egui::TextEdit::singleline(&mut self.cfg.pc_name).desired_width(180.0)).changed();
                });
            });
            card(ui, p, p.bd, |ui| {
                ui.label(text("Подключены сейчас", p.tx).strong());
                let clients = self.host_shared.clients.lock().unwrap().clone();
                if clients.is_empty() {
                    ui.label(text("Никого", p.tx2));
                }
                for cl in clients {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, p.ok);
                        ui.label(text(&cl.name, p.tx));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(text(cl.addr.ip().to_string(), p.tx2)));
                    });
                }
            });
            if changed {
                self.cfg.save();
            }
        });
    }

    // ----- вкладка «Настройки» -----

    fn settings_tab(&mut self, ui: &mut Ui, p: Pal) {
        egui::Frame::new().inner_margin(16).show(ui, |ui| {
            let mut changed = false;
            card(ui, p, p.bd, |ui| {
                row_setting(ui, p, "Тема", "", |ui| {
                    for (key, label) in [("dark", "Тёмная"), ("light", "Светлая"), ("system", "Системная")] {
                        let on = self.cfg.theme == key;
                        let b = egui::Button::new(text(label, if on { p.ac } else { p.tx }))
                            .fill(if on { p.acbg } else { p.card })
                            .stroke(Stroke::new(1.0, if on { p.ac } else { p.bd }));
                        if ui.add(b).clicked() {
                            self.cfg.theme = key.into();
                            ui.ctx().options_mut(|o| o.theme_preference = theme_pref(key));
                            changed = true;
                        }
                    }
                });
                divider(ui, p);
                row_setting(ui, p, "Сворачивать в трей при закрытии", "Окно скрывается, программа продолжает работать", |ui| {
                    changed |= toggle(ui, &mut self.cfg.close_to_tray, p);
                });
                divider(ui, p);
                row_setting(ui, p, "Запускать при входе в Windows", "Сразу в трей", |ui| {
                    let mut on = self.autostart;
                    if toggle(ui, &mut on, p) {
                        winapp::set_autostart(on);
                        self.autostart = winapp::autostart_enabled();
                    }
                });
            });
            card(ui, p, p.bd, |ui| {
                let st = self.updater.state();
                let hint = match &st {
                    UpdState::Idle => "Ещё не проверялось".to_string(),
                    UpdState::Checking => "Проверяю…".to_string(),
                    UpdState::UpToDate => format!("Установлена последняя версия ({VERSION})"),
                    UpdState::Available(r) => format!("Доступна версия {}", r.version),
                    UpdState::Downloading(r) => format!("Скачиваю версию {}…", r.version),
                    UpdState::Error(e) => e.clone(),
                };
                row_setting(ui, p, "Обновления", &hint, |ui| {
                    match &st {
                        UpdState::Available(_) => {
                            if ui.add(primary(p, "Обновить и перезапустить")).clicked() {
                                self.updater.install();
                            }
                        }
                        UpdState::Checking | UpdState::Downloading(_) => {}
                        _ => {
                            if ui.button("Проверить сейчас").clicked() {
                                self.updater.check();
                                self.last_update_check = Instant::now();
                            }
                        }
                    }
                });
                ui.label(text("Проверка раз в сутки. Установщик скачивается с GitHub Releases и проверяется по подписи.", p.tx2).size(12.0));
            });
            ui.label(text(format!("SwitchDesk {VERSION} · github.com/m-dan12/switchdesk"), p.tx2).size(12.0));
            if changed {
                self.cfg.save();
            }
        });
    }
}

fn theme_pref(s: &str) -> ThemePreference {
    match s {
        "dark" => ThemePreference::Dark,
        "light" => ThemePreference::Light,
        _ => ThemePreference::System,
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        ctx.request_repaint_after(Duration::from_millis(250));
        if self.start_hidden {
            self.start_hidden = false;
            winapp::hide_window();
        }
        if self.last_update_check.elapsed() > UPDATE_EVERY {
            self.last_update_check = Instant::now();
            self.updater.check();
        }
        if ctx.input(|i| i.viewport().close_requested()) && self.cfg.close_to_tray && self._tray.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            winapp::hide_window();
        }
        let dark = ctx.theme() == Theme::Dark;
        let p = pal(dark);
        apply_theme(&ctx, p, dark);
        ui.painter().rect_filled(ui.max_rect(), 0, p.bg);

        egui::Panel::top("top").frame(egui::Frame::new().fill(p.side)).show(ui, |ui| {
            self.update_banner(ui, p);
            self.header(ui, p);
        });
        match self.tab {
            Tab::Devices => self.devices_tab(ui, p),
            Tab::Host => self.host_tab(ui, p),
            Tab::Settings => self.settings_tab(ui, p),
        }
    }

    fn on_exit(&mut self) {
        capture::set_sink(None);
        self.cfg.last_device = self.selected;
        self.cfg.save();
    }
}

/// Значок в трее: левый клик открывает окно, меню — быстрые действия.
fn make_tray(client: &Client, updater: &Updater) -> Option<TrayIcon> {
    let menu = Menu::new();
    let open = MenuItem::new("Открыть SwitchDesk", true, None);
    let release = MenuItem::new("Вернуть управление себе", true, None);
    let disconnect = MenuItem::new("Отключиться", true, None);
    let update = MenuItem::new("Проверить обновления", true, None);
    let quit = MenuItem::new("Выход", true, None);
    menu.append_items(&[&open, &release, &disconnect, &update, &PredefinedMenuItem::separator(), &quit]).ok()?;
    let (open_id, release_id, disconnect_id, update_id, quit_id) =
        (open.id().clone(), release.id().clone(), disconnect.id().clone(), update.id().clone(), quit.id().clone());
    let (client, updater) = (client.clone(), updater.clone());
    MenuEvent::set_event_handler(Some(move |e: MenuEvent| {
        if e.id == open_id {
            winapp::show_window();
        } else if e.id == release_id {
            if capture::is_active() {
                capture::toggle();
            }
        } else if e.id == disconnect_id {
            client.disconnect();
        } else if e.id == update_id {
            updater.check();
            winapp::show_window();
        } else if e.id == quit_id {
            capture::set_sink(None);
            std::process::exit(0);
        }
    }));
    TrayIconEvent::set_event_handler(Some(|e: TrayIconEvent| {
        if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = e {
            winapp::show_window();
        }
    }));
    let icon = Icon::from_resource(1, Some((32, 32))).ok()?;
    TrayIconBuilder::new().with_menu(Box::new(menu)).with_tooltip("SwitchDesk").with_icon(icon).build().ok()
}

/// Адреса этого компьютера: LAN и Tailscale (100.64.0.0/10). Виртуальные адаптеры VPN/VM пропускаются.
fn local_addrs() -> Vec<String> {
    const SKIP: [&str; 7] = ["vethernet", "vmware", "virtualbox", "happ", "xray", "wsl", "loopback"];
    let mut v = Vec::new();
    for i in if_addrs::get_if_addrs().unwrap_or_default() {
        let std::net::IpAddr::V4(ip) = i.ip() else { continue };
        let name = i.name.to_lowercase();
        if ip.is_loopback() || ip.is_link_local() || SKIP.iter().any(|s| name.contains(s)) {
            continue;
        }
        let o = ip.octets();
        let tailscale = o[0] == 100 && (64..128).contains(&o[1]);
        v.push(if tailscale { format!("{ip} (Tailscale)") } else { ip.to_string() });
    }
    v.sort_by_key(|a| !a.contains("Tailscale"));
    v
}
