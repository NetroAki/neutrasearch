use std::collections::HashMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::{Arc, atomic::{AtomicU8, AtomicBool, Ordering}};

use sctk::reexports::client::{Connection, Dispatch, Proxy, QueueHandle};
use sctk::reexports::client::globals::GlobalList;
use sctk::reexports::client::protocol::{
    wl_data_device::{self, WlDataDevice},
    wl_data_device_manager::{self, WlDataDeviceManager, DndAction},
    wl_data_offer::{self, WlDataOffer},
    wl_data_source::{self, WlDataSource},
    wl_seat::WlSeat, wl_surface::WlSurface,
};
use sctk::seat::SeatState;
use super::state::WinitState;

#[derive(Debug)]
pub struct DragRequest {
    pub bytes: Arc<Vec<u8>>,
    pub status: Arc<AtomicU8>,
    pub seat: WlSeat,
    pub serial: u32,
    pub surface: WlSurface,
}

pub struct FileDragState {
    manager: Option<WlDataDeviceManager>,
    devices: HashMap<sctk::reexports::client::backend::ObjectId, WlDataDevice>,
    active: Option<WlDataSource>,
    incoming: Option<(WlDataOffer, super::WindowId, crate::dpi::PhysicalPosition<f64>)>,
    pub wake: Option<sctk::reexports::calloop::ping::Ping>,
}

impl FileDragState {
    pub fn new(globals: &GlobalList, qh: &QueueHandle<WinitState>, seats: &SeatState) -> Self {
        let manager: Option<WlDataDeviceManager> = globals.bind(qh, 1..=3, ()).ok();
        let mut devices = HashMap::new();
        if let Some(manager) = &manager {
            for seat in seats.seats() {
                devices.insert(seat.id(), manager.get_data_device(&seat, qh, ()));
            }
        }
        Self { manager, devices, active: None, incoming: None, wake: None }
    }
}

impl WinitState {
    pub fn process_file_drag_requests(&mut self) {
        if self.file_drag.active.as_ref().is_some_and(|source| {
            source.data::<DragRequest>().unwrap().status.load(Ordering::Acquire) >= 2
        }) {
            let source = self.file_drag.active.take().unwrap();
            self.release_drag_button(source.data::<DragRequest>().unwrap());
            source.destroy();
        }
        let requests: Vec<_> = self.window_requests.get_mut().values()
            .filter_map(|requests| requests.drag.lock().unwrap().take()).collect();
        for request in requests {
            if request.status.load(Ordering::Acquire) == 3 { continue; }
            let Some(manager) = &self.file_drag.manager else {
                request.status.store(4, Ordering::Release);
                continue;
            };
            if self.file_drag.active.is_some() {
                request.status.store(4, Ordering::Release);
                continue;
            }
            let qh = self.windows.get_mut().get(&super::make_wid(&request.surface))
                .map(|window| window.lock().unwrap().file_drag_queue());
            let Some(qh) = qh else { request.status.store(4, Ordering::Release); continue; };
            let device = self.file_drag.devices.entry(request.seat.id())
                .or_insert_with(|| manager.get_data_device(&request.seat, &qh, ()));
            let status = request.status.clone();
            let serial = request.serial;
            let surface = request.surface.clone();
            let source = manager.create_data_source(&qh, request);
            source.offer("text/uri-list".into());
            if source.version() >= 3 { source.set_actions(DndAction::Copy); }
            device.start_drag(Some(&source), &surface, None, serial);
            status.store(1, Ordering::Release);
            self.file_drag.active = Some(source);
        }
    }

    fn release_drag_button(&mut self, request: &DragRequest) {
        self.events_sink.push_window_event(crate::event::WindowEvent::MouseInput {
            device_id: crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(super::DeviceId)),
            state: crate::event::ElementState::Released,
            button: crate::event::MouseButton::Left,
        }, super::make_wid(&request.surface));
    }
}

impl Dispatch<WlDataDeviceManager, ()> for WinitState {
    fn event(_: &mut Self, _: &WlDataDeviceManager, _: wl_data_device_manager::Event,
        _: &(), _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<WlDataDevice, ()> for WinitState {
    fn event(state: &mut Self, _: &WlDataDevice, event: wl_data_device::Event,
        _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_device::Event::Enter { serial, surface, x, y, id: Some(offer) } => {
                let accepted = offer.data::<AtomicBool>().unwrap().load(Ordering::Acquire);
                offer.accept(serial, accepted.then(|| "text/uri-list".into()));
                if accepted {
                    if offer.version() >= 3 { offer.set_actions(DndAction::Copy, DndAction::Copy); }
                    let window_id = super::make_wid(&surface);
                    let position = state.drag_position(window_id, x, y);
                    if let Some((stale, _window, _position)) = state.file_drag.incoming.replace((offer, window_id, position)) {
                        stale.destroy();
                    }
                } else { offer.destroy(); }
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                if let Some((_, window, _)) = &state.file_drag.incoming {
                    let window = *window;
                    let position = state.drag_position(window, x, y);
                    if let Some((_, _, stored)) = &mut state.file_drag.incoming { *stored = position; }
                }
            }
            wl_data_device::Event::Drop => {
                if let Some((offer, window, position)) = state.file_drag.incoming.take() {
                    let sink = state.window_events_sink.clone();
                    let wake = state.file_drag.wake.clone();
                    match std::os::unix::net::UnixStream::pair() {
                        Ok((read, write)) => {
                            offer.receive("text/uri-list".into(), std::os::fd::AsFd::as_fd(&write));
                            drop(write);
                            let offer = Arc::new(offer);
                            let sender = offer.clone();
                            if let Err(error) = std::thread::Builder::new().name("file-drag-receive".into()).spawn(move || {
                                match read_offer(read).and_then(|bytes| parse_uri_list(&bytes)) {
                                    Ok(paths) => {
                                        let mut sink = sink.lock().unwrap();
                                        sink.push_window_event(crate::event::WindowEvent::CursorMoved {
                                            device_id: crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(super::DeviceId)),
                                            position,
                                        }, window);
                                        for path in paths { sink.push_window_event(crate::event::WindowEvent::DroppedFile(path), window); }
                                        if sender.version() >= 3 { sender.finish(); }
                                    }
                                    Err(error) => tracing::warn!("file drop rejected: {error}"),
                                }
                                sender.destroy();
                                if let Some(wake) = wake { wake.ping(); }
                            }) { offer.destroy(); tracing::warn!("file drop receiver unavailable: {error}"); }
                        }
                        Err(error) => { offer.destroy(); tracing::warn!("file drop pipe unavailable: {error}"); }
                    }
                }
            }
            wl_data_device::Event::Leave => {
                if let Some((offer, window, _position)) = state.file_drag.incoming.take() {
                    offer.destroy();
                    state.events_sink.push_window_event(crate::event::WindowEvent::HoveredFileCancelled, window);
                }
            }
            wl_data_device::Event::Selection { id: Some(offer) } => offer.destroy(),
            _ => {}
        }
    }
    sctk::reexports::client::event_created_child!(WinitState, WlDataDevice,
        [0 => (WlDataOffer, AtomicBool::new(false))]);
}
impl WinitState {
    fn drag_position(&mut self, window: super::WindowId, x: f64, y: f64) -> crate::dpi::PhysicalPosition<f64> {
        let scale = self.windows.get_mut().get(&window).map(|w| w.lock().unwrap().scale_factor()).unwrap_or(1.0);
        let position = crate::dpi::LogicalPosition::new(x, y).to_physical(scale);
        self.events_sink.push_window_event(crate::event::WindowEvent::CursorMoved {
            device_id: crate::event::DeviceId(crate::platform_impl::DeviceId::Wayland(super::DeviceId)),
            position,
        }, window);
        position
    }
}
impl Dispatch<WlDataOffer, AtomicBool> for WinitState {
    fn event(_: &mut Self, _: &WlDataOffer, event: wl_data_offer::Event,
        data: &AtomicBool, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            if mime_type == "text/uri-list" { data.store(true, Ordering::Release); }
        }
    }
}

fn read_offer(mut stream: std::os::unix::net::UnixStream) -> std::io::Result<Vec<u8>> {
    use std::io::Read;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(15)))?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut bytes = Vec::new();
    let mut block = [0u8; 8192];
    loop {
        if std::time::Instant::now() >= deadline { return Err(std::io::ErrorKind::TimedOut.into()); }
        match stream.read(&mut block) {
            Ok(0) => return Ok(bytes),
            Ok(n) if bytes.len() + n <= 1024 * 1024 => bytes.extend_from_slice(&block[..n]),
            Ok(_) => return Err(std::io::ErrorKind::InvalidData.into()),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}
fn parse_uri_list(bytes: &[u8]) -> std::io::Result<Vec<std::path::PathBuf>> {
    use std::os::unix::ffi::OsStringExt;
    let mut paths = Vec::new();
    for line in bytes.split(|byte| *byte == b'\n') {
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        if line.is_empty() || line.starts_with(b"#") { continue; }
        let path = line.strip_prefix(b"file://").filter(|p| p.starts_with(b"/"))
            .ok_or(std::io::ErrorKind::InvalidData)?;
        let path = percent_encoding::percent_decode(path).collect::<Vec<_>>();
        if path.contains(&0) { return Err(std::io::ErrorKind::InvalidData.into()); }
        paths.push(std::ffi::OsString::from_vec(path).into());
    }
    if paths.is_empty() { return Err(std::io::ErrorKind::InvalidData.into()); }
    Ok(paths)
}

impl Dispatch<WlDataSource, DragRequest> for WinitState {
    fn event(state: &mut Self, source: &WlDataSource, event: wl_data_source::Event,
        data: &DragRequest, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_source::Event::Send { mime_type, fd } if mime_type == "text/uri-list" => {
                let bytes = data.bytes.clone();
                let status = data.status.clone();
                if std::thread::Builder::new().name("file-drag-send".into()).spawn(move || {
                    if write_offer(fd, &bytes).is_err() { status.store(4, Ordering::Release); }
                }).is_err() { data.status.store(4, Ordering::Release); }
            }
            wl_data_source::Event::Cancelled => data.status.store(3, Ordering::Release),
            wl_data_source::Event::DndFinished => data.status.store(2, Ordering::Release),
            _ => {}
        }
        if data.status.load(Ordering::Acquire) >= 2 {
            state.release_drag_button(data);
            source.destroy();
            state.file_drag.active = None;
        }
    }
}

fn write_offer(fd: OwnedFd, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let raw = fd.as_raw_fd();
    // A stalled receiver must not block the display queue or retain a thread.
    unsafe {
        let flags = libc::fcntl(raw, libc::F_GETFL);
        if flags < 0 || libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) < 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    let mut file = std::fs::File::from(fd);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    let mut remaining = bytes;
    while !remaining.is_empty() {
        match file.write(remaining) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => remaining = &remaining[n..],
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if std::time::Instant::now() >= deadline { return Err(std::io::ErrorKind::TimedOut.into()); }
                let mut poll = libc::pollfd { fd: raw, events: libc::POLLOUT, revents: 0 };
                unsafe { libc::poll(&mut poll, 1, 100); }
            }
            Err(e) => return Err(e),
        }
    }
    Ok(())
}
