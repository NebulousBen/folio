pub(crate) fn word_count(text: &str) -> String {
    let count = pdfcraft_engine::word_count::Counts::of(text).words;
    if count == 1 { tl!("1 word").into() } else { tl!("{count} words").replace("{count}", &count.to_string()) }
}

#[cfg(not(target_arch = "wasm32"))]
mod library;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) use library::{ReadingUi, show};
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod overlay;

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct PaperAppearance {
    warmth: u16,
    brightness: u16,
}

impl Default for PaperAppearance {
    fn default() -> Self {
        Self { warmth: 0, brightness: 100 }
    }
}

impl PaperAppearance {
    pub(crate) fn normalized(self) -> Self {
        Self { warmth: self.warmth.min(200), brightness: self.brightness.clamp(5, 150) }
    }

    pub(crate) fn controls(&mut self, ui: &mut egui::Ui) -> bool {
        let before = *self;
        ui.label(tl!("Warm paper"));
        ui.add(egui::Slider::new(&mut self.warmth, 0..=200).suffix("%").text(tl!("Warmth")));
        ui.add(egui::Slider::new(&mut self.brightness, 5..=150).suffix("%").text(tl!("Page brightness")));
        if ui.add_enabled(*self != Self::default(), egui::Button::new(tl!("Reset paper appearance"))).clicked() {
            *self = Self::default();
        }
        *self != before
    }

    pub(crate) fn store_for_frame(self, ctx: &egui::Context) {
        ctx.data_mut(|data| data.insert_temp(egui::Id::unique("folio_paper"), self));
    }

    pub(crate) fn raster(ctx: &egui::Context, image: &mut egui::ColorImage) {
        let appearance: Self = ctx.data(|data| data.get_temp(egui::Id::unique("folio_paper"))).unwrap_or_default();
        appearance.apply(image);
    }

    fn apply(self, image: &mut egui::ColorImage) {
        if self == Self::default() {
            return;
        }
        let depth = f32::from(self.warmth.saturating_sub(100)) / 100.0;
        let opacity = f32::from(self.warmth.min(100)) / 100.0 * 0.65 + depth * 0.35;
        let brightness = f32::from(self.brightness) / 100.0;
        let channel_scale = |from: u8, to: u8| {
            let tint = f32::from(from) * (1.0 - depth) + f32::from(to) * depth;
            brightness * (1.0 - opacity * (1.0 - tint / 255.0))
        };
        let green = channel_scale(166, 115);
        let blue = channel_scale(77, 0);
        for pixel in &mut image.pixels {
            let alpha = pixel.a();
            let adjust = |value: u8, scale: f32| (f32::from(value) * scale).round().min(f32::from(alpha)) as u8;
            *pixel = egui::Color32::from_rgba_premultiplied(adjust(pixel.r(), brightness), adjust(pixel.g(), green), adjust(pixel.b(), blue), alpha);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Color32;

    #[test]
    fn warmth_and_brightness_are_display_only_color_adjustments() {
        let mut image = egui::ColorImage::new([2, 1], vec![Color32::WHITE, Color32::TRANSPARENT]);
        PaperAppearance::default().apply(&mut image);
        assert_eq!(image.pixels, [Color32::WHITE, Color32::TRANSPARENT]);
        PaperAppearance { warmth: 200, brightness: 100 }.apply(&mut image);
        assert_eq!(image.pixels, [Color32::from_rgb(255, 115, 0), Color32::TRANSPARENT]);
        PaperAppearance { warmth: 0, brightness: 50 }.apply(&mut image);
        assert_eq!(image.pixels, [Color32::from_rgb(128, 58, 0), Color32::TRANSPARENT]);
    }
}
