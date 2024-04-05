use std::sync::RwLock;

use chrono::{DateTime, Local};
use rand::Rng;

pub(crate) struct Tracker {
    id: u32,
    init_time: DateTime<Local>,
    color: RwLock<Option<colored::Color>>,
}

impl Default for Tracker {
    fn default() -> Self {
        let id: u32 = rand::thread_rng().gen_range(10000000..99999999);
        Self {
            id,
            init_time: Local::now(),
            color: RwLock::new(None),
        }
    }
}

impl Tracker {
    fn duration(&self) -> std::time::Duration {
        Local::now()
            .signed_duration_since(self.init_time)
            .to_std()
            .unwrap()
    }

    fn get_color(&self) -> colored::Color {
        if let Some(color) = self.color.read().unwrap().as_ref().cloned() {
            return color;
        }

        const BRIGHTNESS: f32 = 1.5;

        let min = |a: f32, b: f32| {
            if a < b {
                a
            } else {
                b
            }
        };

        let mut r = ((self.id & 0xFF0000) >> 16) as u8;
        let mut g = ((self.id & 0x00FF00) >> 8) as u8;
        let mut b = (self.id & 0x0000FF) as u8;
        r = min((r as f32) * BRIGHTNESS, 255 as f32) as u8;
        g = min((g as f32) * BRIGHTNESS, 255 as f32) as u8;
        b = min((b as f32) * BRIGHTNESS, 255 as f32) as u8;
        let color = colored::Color::TrueColor { r, g, b };
        self.color.write().unwrap().replace(color.clone());
        color
    }

    pub(crate) fn to_str(&self) -> String {
        format!("{} {}ms", self.id, self.duration().as_millis())
    }

    pub(crate) fn to_color_str(&self) -> String {
        use colored::Colorize;

        let color = self.get_color();
        format!(
            "{} {}ms",
            self.id.to_string().color(color),
            self.duration().as_millis()
        )
    }
}
