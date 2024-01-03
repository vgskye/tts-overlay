#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")] // hide console window on Windows in release

use std::time::{Duration, Instant};
use std::{fs, io::Cursor, thread::sleep};

use base64::Engine;
use color_eyre::eyre::{eyre, Context as _, Result};
use eframe::egui::{self, Frame, TextEdit, Ui};
use eframe::epaint::ahash::HashMap;
use eframe::epaint::{Color32, FontId};
use reqwest::header::ACCEPT;
use rodio::cpal::traits::HostTrait;
use rodio::{Decoder, DeviceTrait, OutputStreamHandle, Source};
use serde::{Deserialize, Serialize};
use serde_json::json;

const SYNTHESIZE_ENDPOINT: &str = "https://texttospeech.googleapis.com/v1/text:synthesize";
const CONFIG_FILE: &str = "config.toml";

type SynthesizeResponse = HashMap<String, String>;

fn main() -> Result<()> {
    color_eyre::install()?;
    env_logger::init();

    let config_file = fs::read_to_string(CONFIG_FILE)?;
    let config: Configuration = toml::from_str(&config_file)?;

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

    let _ = eframe::run_native(
        "TTS Overlay",
        options,
        Box::new(|_cc| Box::new(OverlayApp::new(config, send))),
    );
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

    fn make_synthesize_request(&self) -> Result<SynthesizeResponse> {
        let client = reqwest::blocking::Client::new();
        let json = json!({
            "input": {
                "text": self.text
              },
              "voice": {
                "languageCode": self.config.gcloud_language,
                "name": self.config.gcloud_voice
              },
              "audioConfig": {
                "audioEncoding": "LINEAR16"
              }
        });

        let req = client
            .post(SYNTHESIZE_ENDPOINT)
            .json(&json)
            .header("X-goog-api-key", self.config.gcloud_token.clone())
            .header(ACCEPT, "application/json")
            .build()?;

        let resp = client
            .execute(req)
            .wrap_err("Couldn't execute request to google!")?;
        let res = resp
            .json()
            .wrap_err("Couldn't decode json response from google!")?;

        Ok(res)
    }

    fn decode_synthesize_response(
        &self,
        resp: SynthesizeResponse,
    ) -> Result<Decoder<Cursor<Vec<u8>>>> {
        let encoded = resp
            .get("audioContent")
            .ok_or(eyre!("Couldn't get audiContent from Google's response!"))?;

        let wav = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .wrap_err("Couldn't decode audio content!")?;

        let decoder = rodio::Decoder::new_wav(Cursor::new(wav))
            .wrap_err("Couldn't create decoder for audio content!")?;

        Ok(decoder)
    }

    fn get_audo_device_handle(&self) -> Result<OutputStreamHandle> {
        let mut output_devices = rodio::cpal::default_host()
            .output_devices()
            .wrap_err("Couldn't find any output devices!")?;

        let found_device = output_devices
            .find(|device| device.name().unwrap_or_default() == self.config.output_device)
            .ok_or(eyre!(
                "Couldn't find your configured device! Are you sure you selected the right one?"
            ))?;

        let (_, handle) = rodio::OutputStream::try_from_device(&found_device).wrap_err(format!(
            "Couldn't created output stram for device {}!",
            found_device.name().unwrap_or_default()
        ))?;

        Ok(handle)
    }

    fn run(&self) -> Result<()> {
        let resp = self.make_synthesize_request()?;
        let decoder = self.decode_synthesize_response(resp)?;
        let output_handle = self.get_audo_device_handle()?;

        let duration = decoder
            .total_duration()
            .unwrap_or(Duration::from_millis(25000));

        output_handle
            .play_raw(decoder.convert_samples())
            .wrap_err("Couldn't play audio!")?;

        // for good measure
        sleep(duration + Duration::from_millis(500));

        Ok(())
    }
}

impl eframe::App for OverlayApp {
    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        [0.; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        let frame = Frame::central_panel(&ctx.style())
            .fill(Color32::TRANSPARENT)
            .inner_margin(4.);

        let show = |ui: &mut Ui| {
            let textbox = TextEdit::singleline(&mut self.text)
                .hint_text("What do you want to say?")
                .font(FontId::proportional(24.))
                .desired_width(f32::INFINITY);

            let textbox = ui.add(textbox);

            if !textbox.has_focus() && self.grace_period <= Instant::now() {
                if ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    if let Some(waiter) = self.waiter.take() {
                        _ = self.run();
                        _ = waiter.send(());
                    };
                } else if let Some(waiter) = self.waiter.take() {
                    _ = waiter.send(());
                }

                ctx.send_viewport_cmd(egui::ViewportCommand::Close)
            } else {
                textbox.request_focus();
            };
        };

        egui::CentralPanel::default().frame(frame).show(ctx, show);
    }
}
