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
