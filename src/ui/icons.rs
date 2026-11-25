use eframe::egui::{self, Color32, Pos2, Rect, Sense, Stroke, Vec2};

pub fn status_dot(ui: &mut egui::Ui, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.0, color);
}

fn icon_btn(ui: &mut egui::Ui, tip: &str, draw: impl FnOnce(&egui::Painter, Rect, Color32)) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(20.0), Sense::click());
    let col = if resp.hovered() {
        ui.visuals().strong_text_color()
    } else {
        ui.visuals().text_color()
    };
    if resp.hovered() {
        ui.painter().rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill);
    }
    draw(ui.painter(), rect.shrink(4.0), col);
    resp.on_hover_text(tip)
}

pub fn pin_button(ui: &mut egui::Ui, pinned: bool, accent: Color32) -> egui::Response {
    icon_btn(ui, if pinned { "Unpin" } else { "Pin" }, |p, r, col| {
        let c = if pinned { accent } else { col };
        let center = r.center();
        if pinned {
            p.circle_filled(Pos2::new(center.x, r.top() + 3.5), 3.0, c);
            p.line_segment([Pos2::new(center.x, r.top() + 6.0), Pos2::new(center.x, r.bottom())], Stroke::new(1.6, c));
        } else {
            p.circle_stroke(Pos2::new(center.x, r.top() + 3.5), 2.8, Stroke::new(1.3, c));
            p.line_segment([Pos2::new(center.x, r.top() + 6.3), Pos2::new(center.x, r.bottom())], Stroke::new(1.3, c));
        }
    })
}

pub fn edit_button(ui: &mut egui::Ui) -> egui::Response {
    icon_btn(ui, "Rename", |p, r, col| {
        p.line_segment([Pos2::new(r.left() + 1.0, r.bottom() - 1.0), Pos2::new(r.right() - 1.0, r.top() + 1.0)], Stroke::new(1.6, col));
        p.line_segment([Pos2::new(r.left(), r.bottom()), Pos2::new(r.left() + 3.5, r.bottom())], Stroke::new(1.4, col));
    })
}

pub fn delete_button(ui: &mut egui::Ui) -> egui::Response {
    icon_btn(ui, "Delete", |p, r, col| {
        let s = Stroke::new(1.5, col);
        p.line_segment([r.left_top(), r.right_bottom()], s);
        p.line_segment([r.right_top(), r.left_bottom()], s);
    })
}

pub fn plus_button(ui: &mut egui::Ui, tip: &str) -> egui::Response {
    icon_btn(ui, tip, |p, r, col| {
        let s = Stroke::new(1.6, col);
        let c = r.center();
        p.line_segment([Pos2::new(r.left(), c.y), Pos2::new(r.right(), c.y)], s);
        p.line_segment([Pos2::new(c.x, r.top()), Pos2::new(c.x, r.bottom())], s);
    })
}

pub fn speaker_button(ui: &mut egui::Ui) -> egui::Response {
    icon_btn(ui, "Read Aloud", |p, r, col| {
        let s = Stroke::new(1.4, col);
        let c = r.center();
        p.rect_filled(Rect::from_min_max(Pos2::new(r.left(), c.y - 2.5), Pos2::new(r.left() + 3.5, c.y + 2.5)), 1.0, col);
        p.line_segment([Pos2::new(r.left() + 3.5, c.y - 2.5), Pos2::new(r.left() + 7.5, r.top() + 1.0)], s);
        p.line_segment([Pos2::new(r.left() + 3.5, c.y + 2.5), Pos2::new(r.left() + 7.5, r.bottom() - 1.0)], s);
        p.line_segment([Pos2::new(r.left() + 7.5, r.top() + 1.0), Pos2::new(r.left() + 7.5, r.bottom() - 1.0)], s);
        p.line_segment([Pos2::new(r.right() - 1.5, c.y - 3.0), Pos2::new(r.right() - 1.5, c.y + 3.0)], s);
    })
}
