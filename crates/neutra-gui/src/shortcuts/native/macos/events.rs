use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, EventLoop},
    platform::{
        macos::{ActivationPolicy, EventLoopBuilderExtMacOS},
        pump_events::EventLoopExtPumpEvents,
    },
    window::WindowId,
};

struct AgentEvents;

impl ApplicationHandler for AgentEvents {
    fn resumed(&mut self, _event_loop: &ActiveEventLoop) {}
    fn window_event(&mut self, _event_loop: &ActiveEventLoop, _id: WindowId, _event: WindowEvent) {}
}

pub(super) fn run() -> Result<(), String> {
    let mut builder = EventLoop::builder();
    builder
        .with_activation_policy(ActivationPolicy::Accessory)
        .with_activate_ignoring_other_apps(false)
        .with_default_menu(false);
    let mut event_loop = builder.build().map_err(|error| error.to_string())?;
    let mut handler = AgentEvents;
    super::super::agent_loop(|timeout| {
        event_loop.pump_app_events(Some(timeout), &mut handler);
    })
}
