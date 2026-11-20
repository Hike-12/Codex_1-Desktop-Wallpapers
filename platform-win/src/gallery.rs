use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};

use core_engine::{Result, library, shader::installed_packs};
use eframe::egui::{self, Color32, RichText, Vec2};

use crate::ipc::{self, Control, State};

const TITLE: &str = "Wallpaper Gallery";
const INK: Color32 = Color32::from_rgb(228, 239, 244);
const MUTED: Color32 = Color32::from_rgb(156, 175, 185);
const PANEL: Color32 = Color32::from_rgb(17, 31, 45);
const ACCENT: Color32 = Color32::from_rgb(65, 214, 191);
const BACKGROUND: Color32 = Color32::from_rgb(11, 20, 30);
const GAP: f32 = 12.0;

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
            visuals.selection.bg_fill = PANEL;
            visuals.selection.stroke.color = ACCENT;
            ctx.set_visuals(visuals);
            let mut gallery = Gallery {
                root,
                cards: Vec::new(),
                state: None,
                pending: None,
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
    state: Option<State>,
    pending: Option<(Receiver<Update>, bool)>,
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
        if !matches!(work, Work::Control(Control::Status)) {
            self.status = None;
        }
        self.pending = Some((rx, importing));
        std::thread::spawn(move || {
            let mut update = Update {
                state: None,
                refresh: false,
                message: None,
            };
            let result = match work {
                Work::Control(control) => ipc::control(control),
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
        let Some((receiver, _)) = &self.pending else {
            return;
        };
        match receiver.try_recv() {
            Ok(update) => {
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
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                self.status = Some("The wallpaper request did not finish".into());
            }
        }
        self.pending = None;
        self.next_poll = Instant::now() + Duration::from_secs(2);
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
                ui.horizontal_wrapped(|ui| {
                    ui.label(RichText::new("Wallpaper Gallery").size(32.0).strong());
                    if ui
                        .add_enabled(
                            self.pending.is_none(),
                            egui::Button::new(
                                RichText::new("+ Upload Video / Image")
                                    .color(BACKGROUND)
                                    .size(16.0),
                            )
                            .fill(ACCENT),
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
                ui.label(
                    RichText::new("Choose a wallpaper or drop an image or video here.")
                        .color(MUTED),
                );
                if let Some(state) = &self.state {
                    ui.add_enabled_ui(self.pending.is_none(), |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui
                                .button(if state.paused { "Resume" } else { "Pause" })
                                .clicked()
                            {
                                work = Some(Work::Control(Control::Pause(!state.paused)));
                            }
                            let mut audio = state.audio;
                            if ui.checkbox(&mut audio, "Audio Reactivity").changed() {
                                work = Some(Work::Control(Control::Audio(audio)));
                            }
                            let mut fps = state.fps;
                            egui::ComboBox::from_id_salt("fps")
                                .selected_text(fps_label(fps))
                                .show_ui(ui, |ui| {
                                    for value in [15, 30, 60] {
                                        ui.selectable_value(&mut fps, value, fps_label(value));
                                    }
                                });
                            if fps != state.fps {
                                work = Some(Work::Control(Control::Fps(fps)));
                            }
                            let mut startup = state.startup;
                            if ui.checkbox(&mut startup, "Start with OS").changed() {
                                work = Some(Work::Control(Control::Startup(startup)));
                            }
                        });
                    });
                }
                if hovering {
                    ui.label(RichText::new("Drop to import and apply").color(ACCENT));
                } else if self
                    .pending
                    .as_ref()
                    .is_some_and(|(_, importing)| *importing)
                {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Importing wallpaper…");
                    });
                }
                if let Some(status) = &self.status {
                    ui.label(RichText::new(status).color(Color32::LIGHT_RED));
                }
                ui.separator();
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
                                        if draw_card(ui, card, width, active).clicked() {
                                            work = Some(Work::Select(card.path.clone()));
                                        }
                                    }
                                });
                            }
                        });
                    });
            });
        if self.pending.is_none() {
            if let Some(work) = work {
                self.start(ctx, work);
            } else if Instant::now() >= self.next_poll {
                self.start(ctx, Work::Control(Control::Status));
            } else {
                ctx.request_repaint_after(self.next_poll.saturating_duration_since(Instant::now()));
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
    let image_height = (width - 12.0) * 9.0 / 16.0;
    let response = ui.add_sized(
        [width, image_height + 53.0],
        egui::Button::new("")
            .fill(PANEL)
            .corner_radius(6)
            .stroke(egui::Stroke::new(
                if active { 2.0_f32 } else { 0.0 },
                ACCENT,
            )),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::Button,
            ui.is_enabled(),
            active,
            &card.name,
        )
    });
    let painter = ui
        .painter()
        .with_clip_rect(response.rect.intersect(ui.clip_rect()));
    let origin = response.rect.min + Vec2::splat(6.0);
    let picture = egui::Rect::from_min_size(origin, Vec2::new(width - 12.0, image_height));
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
            egui::FontId::proportional(14.0),
            MUTED,
        );
    }
    if response.hovered() || response.has_focus() {
        painter.rect_stroke(
            response.rect.shrink(1.0),
            6,
            egui::Stroke::new(2.0_f32, ACCENT),
            egui::StrokeKind::Inside,
        );
    }
    let text = egui::Rect::from_min_max(
        picture.left_bottom() + Vec2::new(6.0, 0.0),
        response.rect.max - Vec2::splat(6.0),
    );
    ui.put(
        text,
        egui::Label::new(RichText::new(&card.name).size(16.0).color(if active {
            ACCENT
        } else {
            INK
        }))
        .truncate()
        .sense(egui::Sense::hover()),
    );
    response
}
