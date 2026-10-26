use crate::client::Status;
use crate::config::{self, Config, Device};
use crate::ipc::{self, Req, State, UpdView};
use crate::winapp;
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, FontId, Layout, Margin, RichText, Sense, Stroke, Theme,
    ThemePreference, Ui,
};
use std::sync::{Arc, Mutex};
use std::time::Duration;

const VERSION: &str = switchdesk_update::CURRENT;

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
    /// Последнее состояние ядра; обновляется потоком опроса.
    snap: Arc<Mutex<State>>,
    addrs: Vec<String>,
    autostart: bool,
    elevated_task: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>, tab: Option<&str>) -> App {
        let cfg = Config::load();
        cc.egui_ctx.options_mut(|o| o.theme_preference = theme_pref(&cfg.theme));
        let snap = Arc::new(Mutex::new(ipc::call(&Req::State).unwrap_or_default()));
        poll(snap.clone(), cc.egui_ctx.clone());
        App {
            selected: cfg.last_device,
            editing: false,
            snap,
            tab: match tab {
                Some("host") => Tab::Host,
                Some("settings") => Tab::Settings,
                _ => Tab::Devices,
            },
            addrs: local_addrs(),
            autostart: winapp::autostart_enabled(),
            elevated_task: winapp::elevated_task_exists(),
            cfg,
        }
    }

    fn state(&self) -> State {
        self.snap.lock().unwrap().clone()
    }

    /// Команда ядру; в ответ приходит свежее состояние.
    fn cmd(&self, req: Req) {
        if let Ok(s) = ipc::call(&req) {
            *self.snap.lock().unwrap() = s;
        }
    }

    // ----- шапка -----

    fn update_banner(&mut self, ui: &mut Ui, p: Pal) {
        let (msg, button, error) = match self.state().update {
            UpdView::Available(v) => (format!("Доступна версия {v} (у вас {VERSION})"), Some("Обновить и перезапустить"), false),
            UpdView::Downloading(v) => (format!("Скачиваю версию {v}…"), None, false),
            UpdView::Error(e) if e.contains("подпис") => (format!("Обновление не удалось: {e}"), None, true),
            _ => return,
        };
        egui::Frame::new().fill(if error { p.bd } else { p.acbg }).inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(text(msg, if error { p.warn } else { p.ac }).size(13.0));
                if let Some(b) = button {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(primary(p, b)).clicked() {
                            self.cmd(Req::UpdateInstall);
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
        let st = self.state();
        let status = st.status.clone();
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
                            self.cmd(Req::Disconnect);
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
                let active = st.active;
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
                                    self.cmd(Req::Toggle);
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
                                self.cmd(Req::Disconnect);
                            }
                        }
                        _ => {
                            if ui.add(primary(p, "Подключиться")).clicked() {
                                self.cfg.last_device = i;
                                self.cfg.save();
                                self.cmd(Req::Connect { dev: self.cfg.devices[i].clone(), me: self.cfg.pc_name.clone() });
                            }
                        }
                    }
                    if !self.editing && ui.button("Изменить").clicked() {
                        self.editing = true;
                    }
                    if connected {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if st.active { "Вернуть управление себе" } else { "Управлять этим компьютером" };
                            if ui.add(primary(p, label)).clicked() {
                                self.cmd(Req::Toggle);
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
            let st = self.state();
            let running = st.host_running;
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
                            self.cfg.host_enabled = enabled;
                            self.cfg.save();
                            self.cmd(Req::HostApply);
                            self.cfg.host_enabled = self.state().host_running;
                            changed = true;
                        }
                    },
                );
                if let Some(e) = &st.host_error {
                    ui.label(text(e, p.warn).size(12.0));
                }
                divider(ui, p);
                let mut allow = self.cfg.host_allow_input;
                row_setting(ui, p, "Разрешить управление", "Выключите, чтобы мгновенно заморозить ввод", |ui| {
                    if toggle(ui, &mut allow, p) {
                        self.cfg.host_allow_input = allow;
                        self.cfg.save();
                        self.cmd(Req::HostApply);
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
                let clients = st.clients.clone();
                if clients.is_empty() {
                    ui.label(text("Никого", p.tx2));
                }
                for cl in clients {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                        ui.painter().circle_filled(r.center(), 4.0, p.ok);
                        ui.label(text(&cl.name, p.tx));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| ui.label(text(&cl.ip, p.tx2)));
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
                row_setting(ui, p, "Запускать при входе в Windows", "Фоновая часть стартует без окна", |ui| {
                    let mut on = self.autostart;
                    if toggle(ui, &mut on, p) {
                        winapp::set_autostart(on);
                        self.autostart = winapp::autostart_enabled();
                    }
                });
                divider(ui, p);
                let st = self.state();
                let hint = if st.elevated {
                    "Запущено с правами администратора: диспетчер задач и игры управляются"
                } else {
                    "Нужно для диспетчера задач и игр с повышенными правами. Запрос UAC один раз"
                };
                row_setting(ui, p, "Запускать от имени администратора", hint, |ui| {
                    let mut on = self.elevated_task;
                    if toggle(ui, &mut on, p) {
                        if winapp::set_elevated(on) {
                            self.elevated_task = on;
                            if on {
                                winapp::set_autostart(false);
                                self.autostart = false;
                                if !st.elevated {
                                    let _ = ipc::call(&Req::RestartElevated);
                                }
                            }
                        }
                    }
                });
            });
            card(ui, p, p.bd, |ui| {
                let st = self.state().update;
                let hint = match &st {
                    UpdView::Idle => "Ещё не проверялось".to_string(),
                    UpdView::Checking => "Проверяю…".to_string(),
                    UpdView::UpToDate => format!("Установлена последняя версия ({VERSION})"),
                    UpdView::Available(v) => format!("Доступна версия {v}"),
                    UpdView::Downloading(v) => format!("Скачиваю версию {v}…"),
                    UpdView::Error(e) => e.clone(),
                };
                row_setting(ui, p, "Обновления", &hint, |ui| {
                    match &st {
                        UpdView::Available(_) => {
                            if ui.add(primary(p, "Обновить и перезапустить")).clicked() {
                                self.cmd(Req::UpdateInstall);
                            }
                        }
                        UpdView::Checking | UpdView::Downloading(_) => {}
                        _ => {
                            if ui.button("Проверить сейчас").clicked() {
                                self.cmd(Req::UpdateCheck);
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
        self.cfg.last_device = self.selected;
        self.cfg.save();
    }
}

/// Опрашивает ядро; если оно пропало надолго (выход из трея), закрывает и окно.
fn poll(snap: Arc<Mutex<State>>, ctx: egui::Context) {
    std::thread::spawn(move || {
        let mut fails = 0;
        loop {
            std::thread::sleep(Duration::from_millis(300));
            match ipc::call(&Req::State) {
                Ok(s) => {
                    fails = 0;
                    let mut g = snap.lock().unwrap();
                    if *g != s {
                        *g = s;
                        ctx.request_repaint();
                    }
                }
                Err(_) => {
                    fails += 1;
                    if fails > 50 {
                        std::process::exit(0);
                    }
                }
            }
        }
    });
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
