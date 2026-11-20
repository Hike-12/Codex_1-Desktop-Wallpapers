use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

use core_engine::{Result, library, shader::installed_packs};
use eframe::egui::{self, Color32, RichText, Vec2};

use crate::ipc::{self, Control, State};

const TITLE: &str = "Wallpaper Gallery";
const INK: Color32 = Color32::from_rgb(240, 244, 248);
const MUTED: Color32 = Color32::from_rgb(136, 148, 163);
const PANEL: Color32 = Color32::from_rgb(18, 20, 26);
const PANEL_HOVER: Color32 = Color32::from_rgb(26, 30, 39);
const PANEL_ACTIVE: Color32 = Color32::from_rgb(19, 27, 34);
const CONTROL_BAR: Color32 = Color32::from_rgb(14, 16, 21);
const ACCENT: Color32 = Color32::from_rgb(45, 212, 191);
const ACCENT_DIM: Color32 = Color32::from_rgba_premultiplied(45, 212, 191, 35);
const BACKGROUND: Color32 = Color32::from_rgb(10, 11, 14);
const GAP: f32 = 14.0;

pub fn run() -> Result<()> {
    let root = library::folder()?;
    eframe::run_native(
        TITLE,
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1040.0, 740.0])
                .with_min_inner_size([360.0, 440.0])
                .with_drag_and_drop(true),
            ..Default::default()
        },
        Box::new(|creation| {
            let ctx = &creation.egui_ctx;
            let mut visuals = egui::Visuals::dark();
            visuals.override_text_color = Some(INK);
            visuals.panel_fill = BACKGROUND;
            visuals.window_fill = PANEL;
            visuals.extreme_bg_color = BACKGROUND;
            visuals.faint_bg_color = CONTROL_BAR;
            visuals.selection.bg_fill = ACCENT_DIM;
            visuals.selection.stroke.color = ACCENT;
            visuals.widgets.noninteractive.bg_fill = PANEL;
            visuals.widgets.noninteractive.bg_stroke = egui::Stroke::NONE;
            visuals.widgets.noninteractive.corner_radius = egui::CornerRadius::same(6);
            visuals.widgets.inactive.bg_fill = CONTROL_BAR;
            visuals.widgets.inactive.bg_stroke = egui::Stroke::NONE;
            visuals.widgets.inactive.corner_radius = egui::CornerRadius::same(6);
            visuals.widgets.hovered.bg_fill = PANEL_HOVER;
            visuals.widgets.hovered.bg_stroke = egui::Stroke::NONE;
            visuals.widgets.hovered.corner_radius = egui::CornerRadius::same(6);
            visuals.widgets.active.bg_fill = PANEL_ACTIVE;
            visuals.widgets.active.bg_stroke = egui::Stroke::NONE;
            visuals.widgets.active.corner_radius = egui::CornerRadius::same(6);
            ctx.set_visuals(visuals);
            let mut gallery = Gallery {
                root,
                cards: Vec::new(),
                delete_prompt: None,
                state: None,
                pending: None,
                status_poll: None,
                status: None,
                next_poll: Instant::now(),
            };
            gallery.refresh(ctx);
            Ok(Box::new(gallery))
        }),
    )?;
    Ok(())
}

struct Card {
    name: String,
    path: PathBuf,
    thumbnail: Option<egui::TextureHandle>,
}

enum Work {
    Control(Control),
    Select(PathBuf),
    Import(Vec<PathBuf>),
}

struct Update {
    state: Option<State>,
    refresh: bool,
    message: Option<String>,
}

struct Gallery {
    root: PathBuf,
    cards: Vec<Card>,
    delete_prompt: Option<(String, PathBuf)>,
    state: Option<State>,
    pending: Option<(Receiver<Update>, bool)>,
    status_poll: Option<Receiver<Update>>,
    status: Option<String>,
    next_poll: Instant,
}

impl Gallery {
    fn refresh(&mut self, ctx: &egui::Context) {
        let packs = match installed_packs(&self.root) {
            Ok(packs) => packs,
            Err(error) => {
                self.status = Some(error.to_string());
                return;
            }
        };
        let mut previous = std::mem::take(&mut self.cards)
            .into_iter()
            .map(|card| (card.path, card.thumbnail))
            .collect::<std::collections::HashMap<_, _>>();
        self.cards = packs
            .into_iter()
            .map(|(name, path)| {
                let path = path.canonicalize().unwrap_or(path);
                let thumbnail = previous.remove(&path).unwrap_or_else(|| {
                    let pixels = library::thumbnail(&path).ok()?.into_rgba8();
                    let size = [pixels.width() as usize, pixels.height() as usize];
                    Some(ctx.load_texture(
                        path.to_string_lossy(),
                        egui::ColorImage::from_rgba_unmultiplied(size, pixels.as_raw()),
                        egui::TextureOptions::LINEAR,
                    ))
                });
                Card {
                    name,
                    path,
                    thumbnail,
                }
            })
            .collect();
    }

    fn start(&mut self, ctx: &egui::Context, work: Work) {
        let (tx, rx) = mpsc::channel();
        let root = self.root.clone();
        let ctx = ctx.clone();
        let importing = matches!(work, Work::Import(_));
        if matches!(work, Work::Control(Control::Status)) {
            self.status_poll = Some(rx);
        } else {
            self.status = None;
            self.status_poll = None;
            self.pending = Some((rx, importing));
        }
        std::thread::spawn(move || {
            let mut update = Update {
                state: None,
                refresh: false,
                message: None,
            };
            let result = match work {
                Work::Control(control) => {
                    update.refresh = matches!(control, Control::Delete(_));
                    ipc::control(control)
                }
                Work::Select(path) => ipc::set(&path).and_then(|()| ipc::control(Control::Status)),
                Work::Import(paths) => {
                    let mut latest = None;
                    let mut errors = Vec::new();
                    for path in paths {
                        match library::import(&root, &path) {
                            Ok(pack) => latest = Some(pack),
                            Err(error) => errors.push(format!("{}: {error}", path.display())),
                        }
                    }
                    update.refresh = latest.is_some();
                    if !errors.is_empty() {
                        update.message = Some(errors.join("\n"));
                    }
                    latest
                        .as_ref()
                        .map_or(Ok(()), |pack| ipc::set(pack))
                        .and_then(|()| ipc::control(Control::Status))
                }
            };
            match result {
                Ok(state) => update.state = Some(state),
                Err(error) => {
                    let message = error.to_string();
                    if let Some(existing) = &mut update.message {
                        existing.push('\n');
                        existing.push_str(&message);
                    } else {
                        update.message = Some(message);
                    }
                }
            }
            let _ = tx.send(update);
            ctx.request_repaint();
        });
    }

    fn finish(&mut self, ctx: &egui::Context) {
        if let Some(receiver) = self.status_poll.take() {
            match receiver.try_recv() {
                Ok(update) => {
                    self.apply_update(ctx, update);
                    self.next_poll = Instant::now() + Duration::from_secs(2);
                }
                Err(TryRecvError::Empty) => self.status_poll = Some(receiver),
                Err(TryRecvError::Disconnected) => {
                    self.next_poll = Instant::now() + Duration::from_secs(2);
                }
            }
        }
        let Some((receiver, _)) = &self.pending else {
            return;
        };
        match receiver.try_recv() {
            Ok(update) => self.apply_update(ctx, update),
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.status = Some("The wallpaper request did not finish".into());
            }
        }
        self.pending = None;
        self.next_poll = Instant::now() + Duration::from_secs(2);
    }

    fn apply_update(&mut self, ctx: &egui::Context, update: Update) {
        if let Some(message) = update.message {
            self.status = Some(message);
        }
        if let Some(state) = update.state {
            self.state = Some(state);
        }
        if update.refresh {
            self.refresh(ctx);
        }
    }
}

impl eframe::App for Gallery {
    fn update(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        self.finish(ctx);
        let mut work = None;
        let drops: Vec<_> = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .collect()
        });
        if !drops.is_empty() {
            if self.pending.is_none() {
                work = Some(Work::Import(drops));
            } else {
                self.status =
                    Some("Wait for the current request before dropping another file".into());
            }
        }
        let hovering = ctx.input(|input| !input.raw.hovered_files.is_empty());
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BACKGROUND).inner_margin(24))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing = Vec2::splat(GAP);
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new("Wallpaper Gallery")
                                    .size(30.0)
                                    .strong()
                                    .color(INK),
                            );
                            if self.cards.iter().any(|c| {
                                self.state.as_ref().and_then(|s| s.active.as_ref()) == Some(&c.path)
                            }) {
                                let badge_resp = ui
                                    .allocate_response(Vec2::new(54.0, 20.0), egui::Sense::hover());
                                let badge_rect = badge_resp.rect;
                                ui.painter().rect_filled(badge_rect, 10, ACCENT_DIM);
                                ui.painter().rect_stroke(
                                    badge_rect,
                                    10,
                                    egui::Stroke::new(1.0_f32, ACCENT),
                                    egui::StrokeKind::Inside,
                                );
                                ui.painter().text(
                                    badge_rect.center(),
                                    egui::Align2::CENTER_CENTER,
                                    "ACTIVE",
                                    egui::FontId::proportional(10.5),
                                    ACCENT,
                                );
                            }
                        });
                        ui.label(
                            RichText::new("Choose a wallpaper or drop an image or video here.")
                                .color(MUTED)
                                .size(13.5),
                        );
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_enabled(
                                self.pending.is_none(),
                                egui::Button::new(
                                    RichText::new("+ Upload Video / Image")
                                        .color(Color32::from_rgb(10, 11, 14))
                                        .size(14.0)
                                        .strong(),
                                )
                                .fill(ACCENT)
                                .corner_radius(8),
                            )
                            .clicked()
                        {
                            let files = rfd::FileDialog::new()
                                .set_parent(frame)
                                .add_filter("Videos and images", library::MEDIA_EXTENSIONS)
                                .pick_files();
                            if let Some(files) = files.filter(|files| !files.is_empty()) {
                                work = Some(Work::Import(files));
                            }
                        }
                    });
                });
                if let Some(state) = &self.state {
                    ui.add_enabled_ui(self.pending.is_none(), |ui| {
                        egui::Frame::new()
                            .fill(CONTROL_BAR)
                            .corner_radius(8)
                            .inner_margin(egui::Margin::symmetric(14, 8))
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    let pause_icon = if state.paused {
                                        "▶ Resume"
                                    } else {
                                        "⏸ Pause"
                                    };
                                    if ui.button(RichText::new(pause_icon).size(13.0)).clicked() {
                                        work = Some(Work::Control(Control::Pause(!state.paused)));
                                    }
                                    let mut audio = state.audio;
                                    if ui
                                        .checkbox(
                                            &mut audio,
                                            RichText::new("Audio Reactivity").size(13.0),
                                        )
                                        .changed()
                                    {
                                        work = Some(Work::Control(Control::Audio(audio)));
                                    }
                                    let mut cursor_effects = state.cursor_effects;
                                    if ui.checkbox(&mut cursor_effects, "Cursor effects").changed()
                                    {
                                        work = Some(Work::Control(Control::CursorEffects(
                                            cursor_effects,
                                        )));
                                    }
                                    let mut fps = state.fps;
                                    egui::ComboBox::from_id_salt("fps")
                                        .selected_text(RichText::new(fps_label(fps)).size(13.0))
                                        .show_ui(ui, |ui| {
                                            for value in [15, 30, 60] {
                                                ui.selectable_value(
                                                    &mut fps,
                                                    value,
                                                    RichText::new(fps_label(value)).size(13.0),
                                                );
                                            }
                                        });
                                    if fps != state.fps {
                                        work = Some(Work::Control(Control::Fps(fps)));
                                    }
                                    let mut startup = state.startup;
                                    if ui
                                        .checkbox(
                                            &mut startup,
                                            RichText::new("Start with OS").size(13.0),
                                        )
                                        .changed()
                                    {
                                        work = Some(Work::Control(Control::Startup(startup)));
                                    }
                                    if ui.button("Return to Windows wallpaper").clicked() {
                                        work = Some(Work::Control(Control::Quit));
                                    }
                                });
                            });
                    });
                }
                if hovering {
                    egui::Frame::new()
                        .fill(ACCENT_DIM)
                        .corner_radius(8)
                        .stroke(egui::Stroke::new(1.5_f32, ACCENT))
                        .inner_margin(egui::Margin::symmetric(14, 8))
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(
                                        "📥 Drop image or video here to import and apply",
                                    )
                                    .color(ACCENT)
                                    .size(13.5)
                                    .strong(),
                                );
                            });
                        });
                } else if self
                    .pending
                    .as_ref()
                    .is_some_and(|(_, importing)| *importing)
                {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(
                            RichText::new("Importing wallpaper…")
                                .color(ACCENT)
                                .size(13.0),
                        );
                    });
                }
                if let Some(status) = &self.status {
                    ui.label(RichText::new(status).color(Color32::LIGHT_RED));
                }
                egui::ScrollArea::vertical()
                    .auto_shrink([false; 2])
                    .show(ui, |ui| {
                        ui.add_enabled_ui(self.pending.is_none(), |ui| {
                            let available = ui.available_width();
                            let columns =
                                ((available + GAP) / (206.0 + GAP)).floor().max(1.0) as usize;
                            let width = (available - (columns - 1) as f32 * GAP) / columns as f32;
                            for row in self.cards.chunks(columns) {
                                ui.horizontal(|ui| {
                                    for card in row {
                                        let active = self
                                            .state
                                            .as_ref()
                                            .and_then(|state| state.active.as_ref())
                                            == Some(&card.path);
                                        let response = draw_card(ui, card, width, active);
                                        let delete_rect = egui::Rect::from_min_size(
                                            response.rect.right_bottom() - Vec2::new(62.0, 34.0),
                                            Vec2::new(54.0, 24.0),
                                        );
                                        if ui
                                            .place(delete_rect, egui::Button::new("Delete"))
                                            .clicked()
                                        {
                                            self.delete_prompt =
                                                Some((card.name.clone(), card.path.clone()));
                                        } else if response.clicked() {
                                            work = Some(Work::Select(card.path.clone()));
                                        }
                                    }
                                });
                            }
                        });
                    });
            });
        if let Some((name, path)) = self.delete_prompt.clone() {
            let mut confirmed = false;
            let mut cancelled = false;
            let modal = egui::Modal::new(egui::Id::new("delete-wallpaper")).show(ctx, |ui| {
                ui.heading("Delete wallpaper?");
                ui.label(format!("“{name}” will be removed from the gallery."));
                ui.horizontal(|ui| {
                    cancelled = ui.button("Cancel").clicked();
                    confirmed = ui.button("Delete wallpaper").clicked();
                });
            });
            if confirmed {
                work = Some(Work::Control(Control::Delete(path)));
            }
            if confirmed || cancelled || modal.should_close() {
                self.delete_prompt = None;
            }
        }
        if self.pending.is_none() {
            if let Some(work) = work {
                self.start(ctx, work);
            } else if self.status_poll.is_none() {
                if Instant::now() >= self.next_poll {
                    self.start(ctx, Work::Control(Control::Status));
                } else {
                    ctx.request_repaint_after(
                        self.next_poll.saturating_duration_since(Instant::now()),
                    );
                }
            }
        }
    }
}

fn fps_label(fps: u8) -> &'static str {
    match fps {
        15 => "15 FPS · Saver",
        60 => "60 FPS · Smooth",
        _ => "30 FPS · Balanced",
    }
}

fn draw_card(ui: &mut egui::Ui, card: &Card, width: f32, active: bool) -> egui::Response {
    let image_height = (width - 16.0) * 9.0 / 16.0;
    let card_height = image_height + 52.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, card_height), egui::Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            active,
            &card.name,
        )
    });
    let is_hovered = response.hovered() || response.has_focus();
    let bg_color = if active {
        PANEL_ACTIVE
    } else if is_hovered {
        PANEL_HOVER
    } else {
        PANEL
    };
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    painter.rect_filled(rect, 8, bg_color);

    let origin = rect.min + Vec2::new(8.0, 8.0);
    let picture = egui::Rect::from_min_size(origin, Vec2::new(width - 16.0, image_height));
    if let Some(thumbnail) = &card.thumbnail {
        let [w, h] = thumbnail.size();
        let aspect = w as f32 / h as f32;
        let crop = Vec2::new(
            (16.0 / 9.0 / aspect).min(1.0),
            (aspect * 9.0 / 16.0).min(1.0),
        );
        painter.image(
            thumbnail.id(),
            picture,
            egui::Rect::from_center_size(egui::pos2(0.5, 0.5), crop),
            Color32::WHITE,
        );
    } else {
        painter.rect_filled(picture, 4, BACKGROUND);
        painter.text(
            picture.center(),
            egui::Align2::CENTER_CENTER,
            "No preview",
            egui::FontId::proportional(13.0),
            MUTED,
        );
    }
    if active {
        let badge_origin = picture.right_top() + Vec2::new(-46.0, 6.0);
        let badge_rect = egui::Rect::from_min_size(badge_origin, Vec2::new(40.0, 18.0));
        painter.rect_filled(
            badge_rect,
            4,
            Color32::from_rgba_premultiplied(10, 11, 14, 210),
        );
        painter.text(
            badge_rect.center(),
            egui::Align2::CENTER_CENTER,
            "ACTIVE",
            egui::FontId::proportional(9.5),
            ACCENT,
        );
    }
    let text_rect = egui::Rect::from_min_max(
        picture.left_bottom() + Vec2::new(2.0, 4.0),
        rect.max - Vec2::new(66.0, 4.0),
    );
    let text_color = if active {
        ACCENT
    } else if is_hovered {
        INK
    } else {
        Color32::from_rgb(205, 215, 225)
    };
    painter
        .with_clip_rect(text_rect.intersect(ui.clip_rect()))
        .text(
            egui::pos2(text_rect.min.x, text_rect.center().y),
            egui::Align2::LEFT_CENTER,
            &card.name,
            egui::FontId::proportional(14.5),
            text_color,
        );
    response
}
