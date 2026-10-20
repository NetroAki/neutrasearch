use super::*;

pub(super) fn initial_points(count: usize, rect: Rect) -> Vec<egui::Pos2> {
    let mut points = vec![rect.center()];
    let radius = rect.width().min(rect.height()) * 0.35;
    for index in 1..count {
        let angle = index as f32 * 2.399_963_1;
        let distance = radius * (index as f32 / count as f32).sqrt();
        points.push(rect.center() + Vec2::new(angle.cos(), angle.sin()) * distance);
    }
    points
}

pub(super) fn relax(state: &mut Simulation, nodes: &[Node], radii: &[f32], rect: Rect) {
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
