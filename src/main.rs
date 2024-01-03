#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::{
    fs,
    io::Cursor,
    thread::{self, sleep},
    time::{Duration, Instant},
};

use base64::Engine;
use eframe::{
    egui::{self, Frame, TextEdit},
    epaint::{ahash::HashMap, Color32, FontId},
};
use reqwest::header::ACCEPT;
use rodio::{cpal::traits::HostTrait, DeviceTrait, Source};
use serde::{Deserialize, Serialize};
use serde_json::json;

fn main() -> Result<(), eframe::Error> {
    env_logger::init();
    let config: Configuration =
        toml::from_str(&fs::read_to_string("config.toml").unwrap()).unwrap();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([config.width, 1.0])
            .with_position([config.x, config.y])
            .with_active(true)
            .with_always_on_top()
            .with_decorations(false)
            .with_transparent(true),
        ..Default::default()
    };
    let (send, recv) = oneshot::channel();
    eframe::run_native(
        "TTS Overlay",
        options,
        Box::new(|_cc| Box::new(OverlayApp::new(config, send))),
    )?;
    _ = recv.recv();
    Ok(())
}

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
struct Configuration {
    font_size: f32,
    width: f32,
    x: f32,
    y: f32,
    gcloud_token: String,
    gcloud_language: String,
    gcloud_voice: String,
    output_device: String,
}

struct OverlayApp {
    text: String,
    grace_period: Instant,
    config: Configuration,
    waiter: Option<oneshot::Sender<()>>,
}

impl OverlayApp {
    fn new(config: Configuration, waiter: oneshot::Sender<()>) -> Self {
        Self {
            text: String::new(),
            grace_period: Instant::now() + Duration::from_millis(500),
            config,
            waiter: Some(waiter),
        }
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.; 4]
    }
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default()
            .frame(
                Frame::central_panel(&ctx.style())
                    .fill(Color32::TRANSPARENT)
                    .inner_margin(4.),
            )
            .show(ctx, |ui| {
                let textbox = TextEdit::singleline(&mut self.text)
                    .hint_text("What do you want to say?")
                    .font(FontId::proportional(24.))
                    .desired_width(f32::INFINITY);
                let textbox = ui.add(textbox);
                if !textbox.has_focus() && self.grace_period <= Instant::now() {
                    if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        if let Some(waiter) = self.waiter.take() {
                            let text = self.text.clone();
                            let config = self.config.clone();
                            thread::spawn(move || {
                                let client = reqwest::blocking::Client::new();
                                if let Ok(resp) = client
                                    .post("https://texttospeech.googleapis.com/v1/text:synthesize")
                                    .json(&json!({
                                      "input": {
                                        "text": text
                                      },
                                      "voice": {
                                        "languageCode": config.gcloud_language,
                                        "name": config.gcloud_voice
                                      },
                                      "audioConfig": {
                                        "audioEncoding": "LINEAR16"
                                      }
                                    }))
                                    .header("X-goog-api-key", config.gcloud_token)
                                    .header(ACCEPT, "application/json")
                                    .send()
                                {
                                    if let Ok(value) = resp.json::<HashMap<String, String>>() {
                                        if let Some(encoded) = value.get("audioContent") {
                                            if let Ok(wav) =
                                                base64::engine::general_purpose::STANDARD
                                                    .decode(encoded)
                                            {
                                                let host = rodio::cpal::default_host();
                                                if let Ok(devices) = host.output_devices() {
                                                    for device in devices {
                                                        if let Ok(name) = device.name() {
                                                            if name.contains(&config.output_device)
                                                            {
                                                                if let Ok((_, handle)) =
                                                                    rodio::OutputStream::try_from_device(&device)
                                                                {
                                                                    if let Ok(decoder) =
                                                                        rodio::Decoder::new_wav(Cursor::new(wav))
                                                                    {
                                                                        if let Some(duration) =
                                                                            decoder.total_duration()
                                                                        {
                                                                            if let Ok(()) = handle
                                                                                .play_raw(decoder.convert_samples())
                                                                            {
                                                                                // for good measure
                                                                                sleep(
                                                                                    duration
                                                                                        + Duration::from_millis(
                                                                                            500,
                                                                                        ),
                                                                                );
                                                                            };
                                                                        }
                                                                    };
                                                                }
                                                                break;
                                                            }
                                                        }
                                                    }
                                                };
                                            }
                                        }
                                    }
                                }
                                _ = waiter.send(());
                            });
                        };
                    } else if let Some(waiter) = self.waiter.take() {
                        _ = waiter.send(());
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close)
                } else {
                    textbox.request_focus();
                }
            });
    }
}
