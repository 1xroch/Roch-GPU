//! Hardware and driver details for the selected GPU.

use eframe::egui::{self, Align, Context, Layout, RichText, ViewportId, vec2};
use roch_core::{GpuGraphicsInfo, GpuGraphicsRow};

use super::{ChildChrome, Placement, WinCtx, content_frame};
use crate::gui::theme::{self, BODY, pal};

pub struct InfoWindow {
    rows: Vec<GpuGraphicsRow>,
    chrome: ChildChrome,
}

impl InfoWindow {
    pub const TITLE: &str = "Roch GPU — Info";

    pub fn id() -> ViewportId {
        ViewportId::from_hash_of("roch_info")
    }

    pub fn new(info: &GpuGraphicsInfo) -> Self {
        let mut window = Self { rows: Vec::new(), chrome: ChildChrome::default() };
        window.set_info(info);
        window
    }

    pub fn set_info(&mut self, info: &GpuGraphicsInfo) {
        self.rows = info.rows();
        self.rows.retain(|row| row.value != "—" && !row.value.eq_ignore_ascii_case("unavailable"));
    }

    pub fn show(&mut self, ctx: &Context, w: &mut WinCtx) -> bool {
        let builder = self
            .chrome
            .builder(Self::TITLE, vec2(620.0, 525.0), w.icon, Placement::RightOfOwner, w.main_rect)
            .with_min_inner_size(self.chrome.min_size(vec2(420.0, 300.0)))
            .with_taskbar(false);
        ctx.show_viewport_immediate(Self::id(), builder, |ui, _| {
            let closed = self.chrome.frame(ui, w.dark);
            let frame = content_frame(ui.ctx(), 14);
            egui::CentralPanel::default().frame(frame).show(ui, |ui| {
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| self.body(ui));
            });
            !closed
        })
    }

    fn body(&self, ui: &mut egui::Ui) {
        let p = pal(ui.ctx());
        if self.rows.is_empty() {
            ui.label(RichText::new("No GPU information available.").font(theme::regular(BODY)).color(p.muted));
        }
        for (index, row) in self.rows.iter().enumerate() {
            let fill = if index % 2 == 1 { p.highlight } else { egui::Color32::TRANSPARENT };
            egui::Frame::new().fill(fill).inner_margin(egui::Margin::symmetric(8, 4)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_top(|ui| {
                    let label_width = ui.available_width() * 0.38;
                    ui.allocate_ui_with_layout(vec2(label_width, 0.0), Layout::top_down(Align::Min), |ui| {
                        ui.set_min_width(label_width);
                        ui.add(
                            egui::Label::new(RichText::new(&row.name).font(theme::regular(BODY)).color(p.text)).wrap(),
                        );
                    });
                    ui.add(
                        egui::Label::new(RichText::new(&row.value).font(theme::regular(BODY)).color(p.accent)).wrap(),
                    );
                });
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_shows_reported_fields_and_refreshes_for_the_selected_gpu() {
        let info = GpuGraphicsInfo {
            gpu: "Intel Arc Pro B60".into(),
            board_manufacturer: "ASRock".into(),
            resizable_bar: "Unavailable".into(),
            ..Default::default()
        };
        let mut window = InfoWindow::new(&info);
        let shown: Vec<&str> = window.rows.iter().map(|row| row.value.as_str()).collect();
        assert_eq!(shown, ["Intel Arc Pro B60", "ASRock"]);

        window.set_info(&GpuGraphicsInfo { gpu: "AMD Radeon RX 7900 XT".into(), ..Default::default() });
        assert_eq!(window.rows.len(), 1);
        assert_eq!(window.rows[0].value, "AMD Radeon RX 7900 XT");
    }
}
