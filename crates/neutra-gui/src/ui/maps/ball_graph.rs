use super::super::*;
use std::hash::{Hash, Hasher};

const MAX_NODES: usize = 128;

struct Node {
    path: String,
    parent: Option<usize>,
    bytes: u64,
    folder: bool,
}

#[derive(Clone)]
struct Simulation {
    key: u64,
    points: Vec<egui::Pos2>,
    velocity: Vec<Vec2>,
    steps: usize,
}

pub(super) fn show(
    ui: &mut Ui,
    model: &Hierarchy,
    current: &str,
    selected: Option<&str>,
) -> Option<TreeAction> {
    let nodes = nodes(model, current);
    ui.label(
        RichText::new(format!(
            "{} cached items shown · up to {MAX_NODES} largest hierarchy nodes",
            nodes.len()
        ))
        .font(sans(CAPTION))
        .color(MUTED),
    );
    ui.label(RichText::new("Drag to arrange; hold Ctrl to copy a file into a folder. Double-click to open; right-click for actions.")
        .font(sans(CAPTION)).color(MUTED));
    let (rect, _) =
        ui.allocate_exact_size(ui.available_size().max(Vec2::splat(1.0)), Sense::hover());
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    current.hash(&mut hasher);
    for node in &nodes {
        node.path.hash(&mut hasher);
        node.bytes.hash(&mut hasher);
    }
    [rect.width().round() as i32, rect.height().round() as i32].hash(&mut hasher);
    let key = hasher.finish();
    let id = Id::new("ball-simulation");
    let mut simulation = ui
        .ctx()
        .data(|data| data.get_temp::<Simulation>(id))
        .filter(|state| state.key == key)
        .unwrap_or_else(|| Simulation {
            key,
            points: initial_points(nodes.len(), rect),
            velocity: vec![Vec2::ZERO; nodes.len()],
            steps: 0,
        });
    let max = nodes
        .iter()
        .map(|node| node.bytes)
        .max()
        .unwrap_or(1)
        .max(1) as f64;
    let radii: Vec<f32> = nodes
        .iter()
        .map(|node| ((node.bytes as f64 / max).sqrt() as f32 * 42.0).max(3.0))
        .collect();
    let active = simulation.steps < 160;
    if active {
        relax(&mut simulation, &nodes, &radii, rect);
        simulation.steps += 1;
    }
    for (index, node) in nodes.iter().enumerate().skip(1) {
        if let Some(parent) = node.parent {
            ui.painter().line_segment(
                [simulation.points[parent], simulation.points[index]],
                Stroke::new(1.0_f32, MUTED.gamma_multiply(0.5)),
            );
        }
    }
    let mut action = None;
    let mut dragged = false;
    for (index, node) in nodes.iter().enumerate() {
        let point = simulation.points[index];
        let radius = radii[index];
        let hit = Rect::from_center_size(point, Vec2::splat((radius * 2.0).max(12.0)));
        let response = ui.interact(hit, Id::new(("ball", &node.path)), Sense::click_and_drag());
        let copying = ui.input(|input| input.modifiers.command);
        if response.dragged() && !copying {
            simulation.points[index] += response.drag_delta();
            simulation.velocity[index] = Vec2::ZERO;
            simulation.steps = 0;
            dragged = true;
        }
        let color = if node.folder {
            WARN
        } else {
            extension_color(
                path_name(&node.path)
                    .rsplit_once('.')
                    .map_or("", |(_, ext)| ext),
            )
        };
        ui.painter().circle_filled(
            point,
            radius,
            color.gamma_multiply(if response.hovered() { 0.55 } else { 0.3 }),
        );
        ui.painter().circle_stroke(
            point,
            radius,
            Stroke::new(
                if selected == Some(node.path.as_str()) || response.has_focus() {
                    2.0_f32
                } else {
                    1.0_f32
                },
                if selected == Some(node.path.as_str()) || response.has_focus() {
                    GLOW
                } else {
                    color
                },
            ),
        );
        if radius > 17.0 {
            ui.painter().text(
                point,
                Align2::CENTER_CENTER,
                shorten(&path_name(&node.path), 10),
                sans(MICRO),
                TEXT,
            );
        }
        response
            .clone()
            .on_hover_text(format!("{}\n{}", node.path, format_size(node.bytes)));
        if response.clicked() {
            action = Some(TreeAction::Select(node.path.clone()));
        }
        if response.double_clicked() {
            action = Some(if node.folder {
                TreeAction::Navigate(node.path.clone())
            } else {
                TreeAction::Open(node.path.clone())
            });
        }
        response.context_menu(|menu| file_context_menu(menu, &node.path, &mut action));
        if copying
            || !ui.input(|input| input.raw.dropped_files.is_empty())
            || response
                .dnd_hover_payload::<Vec<std::path::PathBuf>>()
                .is_some()
        {
            if let Some(file_action) =
                super::super::file_interactions::interact(ui, &response, &node.path, node.folder)
            {
                action = Some(TreeAction::FileOperation(file_action));
            }
        }
    }
    let moving = simulation
        .velocity
        .iter()
        .any(|velocity| velocity.length_sq() > 0.04);
    if !dragged && !moving && simulation.steps > 12 {
        simulation.steps = 160;
    }
    if active && moving || dragged {
        ui.ctx()
            .request_repaint_after(std::time::Duration::from_millis(33));
    }
    ui.ctx().data_mut(|data| data.insert_temp(id, simulation));
    action
}

fn nodes(model: &Hierarchy, current: &str) -> Vec<Node> {
    let mut nodes = vec![Node {
        path: current.into(),
        parent: None,
        bytes: model.folders.get(current).map_or(0, |folder| folder.size),
        folder: true,
    }];
    let mut parent = 0;
    while parent < nodes.len() && nodes.len() < MAX_NODES {
        if let Some(folder) = nodes[parent]
            .folder
            .then(|| model.folders.get(&nodes[parent].path))
            .flatten()
        {
            let mut children: Vec<_> = folder
                .children
                .iter()
                .map(|child| (child.size, child.path.clone(), true))
                .chain(
                    folder
                        .direct_files
                        .iter()
                        .map(|file| (file.size, file.path.clone(), false)),
                )
                .collect();
            children.sort_unstable_by_key(|item| std::cmp::Reverse(item.0));
            for (bytes, path, folder) in children.into_iter().take(MAX_NODES - nodes.len()) {
                if !nodes.iter().any(|node| node.path == path) {
                    nodes.push(Node {
                        path,
                        parent: Some(parent),
                        bytes,
                        folder,
                    });
                }
            }
        }
        parent += 1;
    }
    nodes
}

fn initial_points(count: usize, rect: Rect) -> Vec<egui::Pos2> {
    let mut points = vec![rect.center()];
    let radius = rect.width().min(rect.height()) * 0.35;
    for index in 1..count {
        let angle = index as f32 * 2.399_963_1;
        let distance = radius * (index as f32 / count as f32).sqrt();
        points.push(rect.center() + Vec2::new(angle.cos(), angle.sin()) * distance);
    }
    points
}

fn relax(state: &mut Simulation, nodes: &[Node], radii: &[f32], rect: Rect) {
    let mut forces = vec![Vec2::ZERO; nodes.len()];
    for index in 0..nodes.len() {
        for other in index + 1..nodes.len() {
            let delta = state.points[index] - state.points[other];
            let distance = delta.length().max(0.01);
            let direction = if delta.length_sq() > 0.001 {
                delta / distance
            } else {
                Vec2::new(1.0, 0.0)
            };
            let collision = (radii[index] + radii[other] + 5.0 - distance).max(0.0);
            let force = direction * (collision * 0.15 + (100.0 / (distance * distance)).min(1.0));
            forces[index] += force;
            forces[other] -= force;
        }
        if let Some(parent) = nodes[index].parent {
            let delta = state.points[parent] - state.points[index];
            let length = delta.length().max(0.01);
            let target = radii[parent] + radii[index] + 42.0;
            let force = delta / length * (length - target) * 0.012;
            forces[index] += force;
            forces[parent] -= force;
        }
    }
    state.points[0] = rect.center();
    for (index, force) in forces.into_iter().enumerate().skip(1) {
        let velocity = state.velocity[index] + force;
        state.velocity[index] = velocity / (velocity.length() / 4.0).max(1.0) * 0.72;
        state.points[index] += state.velocity[index];
        let radius = radii[index].min(rect.width().min(rect.height()) / 2.0);
        state.points[index].x = state.points[index]
            .x
            .clamp(rect.left() + radius, rect.right() - radius);
        state.points[index].y = state.points[index]
            .y
            .clamp(rect.top() + radius, rect.bottom() - radius);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relaxation_stays_finite_and_inside_the_canvas() {
        let rect = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(320.0, 200.0));
        let nodes = vec![
            Node {
                path: "/".into(),
                parent: None,
                bytes: 100,
                folder: true,
            },
            Node {
                path: "/a".into(),
                parent: Some(0),
                bytes: 10,
                folder: false,
            },
        ];
        let mut state = Simulation {
            key: 0,
            points: vec![rect.center(); 2],
            velocity: vec![Vec2::ZERO; 2],
            steps: 0,
        };
        for _ in 0..160 {
            relax(&mut state, &nodes, &[42.0, 13.0], rect);
        }
        assert!(state
            .points
            .iter()
            .all(|point| point.x.is_finite() && point.y.is_finite() && rect.contains(*point)));
    }
}
