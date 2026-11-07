//! Janela mínima do Zoomcast (egui/eframe). Lógica testável em `logic`.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod logic;

use eframe::egui;
use logic::{Form, TargetKind};
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{SystemTime, UNIX_EPOCH},
};
use zoomcast_capture::{default_backend, list_audio_devices};
use zoomcast_core::project::Project;
use zoomcast_export::{ExportOptions, export_project};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Starting,
    Recording,
    Finishing,
    Exporting,
    Listing,
}

#[derive(Debug)]
enum Msg {
    Started(Result<(), String>),
    Finished(Result<String, String>),
    Exported(Result<String, String>),
    Devices(Result<Vec<String>, String>),
}

struct App {
    form: Form,
    devices: Vec<String>,
    phase: Phase,
    status: String,
    error: Option<String>,
    rx: Receiver<Msg>,
    tx: Sender<Msg>,
    /// Sinal de parada para a thread de gravação.
    stop_tx: Option<Sender<()>>,
}

impl App {
    fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            form: Form::default(),
            devices: Vec::new(),
            phase: Phase::Idle,
            status: "pronto".into(),
            error: None,
            rx,
            tx,
            stop_tx: None,
        }
    }

    fn fail(&mut self, e: String) {
        self.error = Some(e);
        self.phase = Phase::Idle;
        self.status = "erro".into();
    }

    fn refresh_devices(&mut self, ctx: &egui::Context) {
        self.phase = Phase::Listing;
        self.status = "listando microfones…".into();
        self.error = None;
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let r = list_audio_devices(&logic::ffmpeg_bin()).map_err(|e| e.to_string());
            let _ = tx.send(Msg::Devices(r));
            ctx.request_repaint();
        });
    }

    fn record(&mut self, ctx: &egui::Context) {
        self.error = None;
        if self.form.dir.trim().is_empty() {
            let secs = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_secs());
            self.form.dir = logic::auto_dir(secs).display().to_string();
        }
        let (cfg, zoom) = match logic::build_config(&self.form, logic::ffmpeg_bin()) {
            Ok(v) => v,
            Err(e) => return self.fail(e),
        };
        if let Err(e) = logic::check_dir_free(&cfg.out_dir) {
            return self.fail(e);
        }
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        self.stop_tx = Some(stop_tx);
        self.phase = Phase::Starting;
        self.status = "iniciando captura…".into();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let mut backend = default_backend();
            if let Err(e) = backend.start(&cfg) {
                let _ = tx.send(Msg::Started(Err(e.to_string())));
                ctx.request_repaint();
                return;
            }
            let _ = tx.send(Msg::Started(Ok(())));
            ctx.request_repaint();
            // Bloqueia até Parar (ou até a janela fechar: Sender solto → Err, também para).
            let _ = stop_rx.recv();
            let r = (|| {
                let captured = backend.stop().map_err(|e| e.to_string())?;
                let ms = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_err(|e| e.to_string())?
                    .as_millis() as u64;
                let manifest = logic::build_manifest(&captured, zoom, ms)?;
                let project = Project::create(&cfg.out_dir, manifest, &captured.events)
                    .map_err(|e| e.to_string())?;
                Ok(format!(
                    "gravado: {} eventos, {} zooms",
                    captured.events.events.len(),
                    project.manifest.timeline.zooms.len()
                ))
            })();
            let _ = tx.send(Msg::Finished(r));
            ctx.request_repaint();
        });
    }

    fn stop(&mut self) {
        if let Some(s) = self.stop_tx.take() {
            let _ = s.send(());
            self.phase = Phase::Finishing;
            self.status = "finalizando…".into();
        }
    }

    fn export(&mut self, ctx: &egui::Context) {
        self.error = None;
        let root = match logic::parse_dir(&self.form.dir) {
            Ok(r) => r,
            Err(e) => return self.fail(e),
        };
        let opts = ExportOptions {
            ffmpeg_bin: logic::ffmpeg_bin(),
            ..ExportOptions::default()
        };
        self.phase = Phase::Exporting;
        self.status = "exportando…".into();
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        thread::spawn(move || {
            let r = (|| {
                let project = Project::open(&root).map_err(|e| e.to_string())?;
                let out: PathBuf = root.join("render.mp4");
                let rep = export_project(&project, &out, &opts, &mut |_| {})
                    .map_err(|e| e.to_string())?;
                Ok(format!("{}: {} frames", out.display(), rep.frames))
            })();
            let _ = tx.send(Msg::Exported(r));
            ctx.request_repaint();
        });
    }

    fn poll(&mut self) {
        while let Ok(m) = self.rx.try_recv() {
            match m {
                Msg::Started(Ok(())) => {
                    self.phase = Phase::Recording;
                    self.status = "gravando…".into();
                }
                Msg::Started(Err(e)) => {
                    self.stop_tx = None;
                    self.fail(e);
                }
                Msg::Finished(Ok(s)) | Msg::Exported(Ok(s)) => {
                    self.phase = Phase::Idle;
                    self.status = s;
                }
                Msg::Finished(Err(e)) | Msg::Exported(Err(e)) | Msg::Devices(Err(e)) => {
                    self.fail(e);
                }
                Msg::Devices(Ok(d)) => {
                    self.phase = Phase::Idle;
                    self.status = if d.is_empty() {
                        "nenhum microfone encontrado".into()
                    } else {
                        format!("{} microfone(s)", d.len())
                    };
                    if self.form.mic.as_ref().is_some_and(|m| !d.contains(m)) {
                        self.form.mic = None;
                    }
                    self.devices = d;
                }
            }
        }
    }
}

const ACCENT: egui::Color32 = egui::Color32::from_rgb(0x2f, 0x6b, 0xff);

fn style(ctx: &egui::Context) {
    ctx.set_visuals(egui::Visuals::light());
    ctx.style_mut(|s| {
        s.visuals.selection.bg_fill = ACCENT;
        s.visuals.widgets.hovered.bg_stroke.color = ACCENT;
        s.spacing.item_spacing = egui::vec2(10.0, 10.0);
        s.spacing.button_padding = egui::vec2(14.0, 8.0);
        for w in [
            &mut s.visuals.widgets.inactive,
            &mut s.visuals.widgets.hovered,
            &mut s.visuals.widgets.active,
        ] {
            w.corner_radius = egui::CornerRadius::same(10);
        }
    });
}

fn open_folder(dir: &str) {
    let cmd = if cfg!(windows) {
        "explorer"
    } else {
        "xdg-open"
    };
    let _ = std::process::Command::new(cmd).arg(dir).spawn();
}

fn card(ui: &mut egui::Ui, title: &str, body: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::group(ui.style())
        .corner_radius(egui::CornerRadius::same(14))
        .inner_margin(14.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.label(egui::RichText::new(title).strong().size(15.0));
            body(ui);
        });
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll();
        let idle = self.phase == Phase::Idle;
        let rec = self.phase == Phase::Recording;
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.add_enabled_ui(idle, |ui| {
                card(ui, "Fonte", |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(
                            &mut self.form.kind,
                            TargetKind::Primary,
                            "Tela principal",
                        );
                        ui.selectable_value(
                            &mut self.form.kind,
                            TargetKind::Monitor,
                            "Outro monitor",
                        );
                        ui.selectable_value(&mut self.form.kind, TargetKind::Window, "Janela");
                    });
                    match self.form.kind {
                        TargetKind::Primary => {}
                        TargetKind::Monitor => {
                            ui.horizontal(|ui| {
                                ui.label("Número (0 = primeiro)");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.form.monitor)
                                        .desired_width(60.0),
                                );
                            });
                        }
                        TargetKind::Window => {
                            ui.add(
                                egui::TextEdit::singleline(&mut self.form.window)
                                    .hint_text("Título exato da janela")
                                    .desired_width(f32::INFINITY),
                            );
                        }
                    }
                });
                card(ui, "Efeitos", |ui| {
                    ui.add(
                        egui::Slider::new(&mut self.form.zoom, 1.0..=4.0).text("Zoom no clique"),
                    );
                    ui.horizontal(|ui| {
                        ui.label("Microfone");
                        egui::ComboBox::from_id_salt("mic")
                            .selected_text(
                                self.form.mic.clone().unwrap_or_else(|| "Sem áudio".into()),
                            )
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut self.form.mic, None, "Sem áudio");
                                for d in &self.devices {
                                    ui.selectable_value(&mut self.form.mic, Some(d.clone()), d);
                                }
                            });
                        if ui.button("Atualizar").clicked() {
                            self.refresh_devices(ctx);
                        }
                    });
                });
                card(ui, "Salvar em", |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.form.dir)
                            .hint_text("Vazio = Vídeos\\Zoomcast\\rec-<data> (automático)")
                            .desired_width(f32::INFINITY),
                    );
                });
            });
            ui.add_space(6.0);
            ui.vertical_centered(|ui| {
                let (label, fill) = if rec {
                    ("■  Parar", egui::Color32::from_rgb(0x44, 0x44, 0x4c))
                } else {
                    ("●  Gravar", egui::Color32::from_rgb(0xe5, 0x39, 0x35))
                };
                let btn = egui::Button::new(
                    egui::RichText::new(label)
                        .size(20.0)
                        .strong()
                        .color(egui::Color32::WHITE),
                )
                .fill(fill)
                .min_size(egui::vec2(220.0, 52.0))
                .corner_radius(egui::CornerRadius::same(26));
                if ui.add_enabled(idle || rec, btn).clicked() {
                    if rec { self.stop() } else { self.record(ctx) }
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        idle && !self.form.dir.trim().is_empty(),
                        egui::Button::new("Exportar MP4")
                            .fill(ACCENT)
                            .stroke(egui::Stroke::NONE),
                    )
                    .clicked()
                {
                    self.export(ctx);
                }
                if ui
                    .add_enabled(
                        !self.form.dir.trim().is_empty(),
                        egui::Button::new("Abrir pasta"),
                    )
                    .clicked()
                {
                    open_folder(self.form.dir.trim());
                }
            });
            ui.separator();
            ui.label(egui::RichText::new(&self.status).weak());
            if let Some(e) = &self.error {
                ui.colored_label(
                    egui::Color32::from_rgb(0xc6, 0x28, 0x28),
                    format!("Erro: {e}"),
                );
            }
        });
    }
}

fn main() -> eframe::Result {
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default().with_inner_size([460.0, 620.0]),
        ..Default::default()
    };
    eframe::run_native(
        "Zoomcast",
        opts,
        Box::new(|cc| {
            style(&cc.egui_ctx);
            Ok(Box::new(App::new()))
        }),
    )
}
