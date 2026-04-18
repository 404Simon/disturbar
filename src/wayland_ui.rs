use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::constants::{
    BAR_HEIGHT, BAR_HIDDEN_RGBA, BAR_VISIBLE_RGBA, BATTERY_REFRESH_MS, DATETIME_REFRESH_HIDDEN_MS,
    DATETIME_REFRESH_VISIBLE_MS, LOOP_SLEEP_HIDDEN_MS, LOOP_SLEEP_VISIBLE_MS, MARGIN_SIDE,
    MARGIN_TOP, SIGNAL_DETAIL_OFF, SIGNAL_DETAIL_ON, SIGNAL_HIDE, SIGNAL_SHOW, SONG_POLL_HIDDEN_MS,
    SONG_POLL_VISIBLE_MS, TEXT_RGBA, VOLUME_POLL_HIDDEN_MS, VOLUME_POLL_VISIBLE_MS,
    WORKSPACE_POLL_HIDDEN_MS, WORKSPACE_POLL_VISIBLE_MS,
};
use crate::renderer::{self, ShmBarBuffer};
use crate::signals;
use crate::status::{self, BarStatus, StatusEvent, StatusEventStreams, WorkspaceStatus};

pub fn run_wayland_bar() -> Result<(), String> {
    signals::register_signal_handlers();

    let conn = Connection::connect_to_env().map_err(|e| format!("connect wayland: {e}"))?;
    let (globals, mut event_queue) =
        registry_queue_init::<AppState>(&conn).map_err(|e| format!("registry init: {e}"))?;
    let qh = event_queue.handle();

    let compositor = globals
        .bind::<wl_compositor::WlCompositor, _, _>(&qh, 4..=6, ())
        .map_err(|e| format!("bind wl_compositor: {e}"))?;
    let shm = globals
        .bind::<wl_shm::WlShm, _, _>(&qh, 1..=1, ())
        .map_err(|e| format!("bind wl_shm: {e}"))?;
    let layer_shell = globals
        .bind::<zwlr_layer_shell_v1::ZwlrLayerShellV1, _, _>(&qh, 1..=4, ())
        .map_err(|e| format!("bind zwlr_layer_shell_v1: {e}"))?;

    let streams = status::spawn_status_event_streams();
    let mut state = AppState::new(compositor, layer_shell, shm, qh, streams);

    for global in globals.contents().clone_list() {
        if global.interface == "wl_output" {
            state.add_output(globals.registry(), global.name, global.version);
        }
    }

    while state.has_unconfigured_monitors() {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("dispatch init events: {e}"))?;
    }

    if state.visible {
        state.tick_status();
    }
    state.redraw();

    loop {
        let signal = signals::take_visibility_signal();
        if signal & SIGNAL_SHOW != 0 && !state.visible {
            state.visible = true;
            state.mark_all_for_redraw();
        }
        if signal & SIGNAL_HIDE != 0 && state.visible {
            state.visible = false;
            state.mark_all_for_redraw();
        }
        if signal & SIGNAL_DETAIL_ON != 0 && !state.detail_mode {
            state.detail_mode = true;
            state.refresh_detail_status();
        }
        if signal & SIGNAL_DETAIL_OFF != 0 && state.detail_mode {
            state.detail_mode = false;
            state.refresh_detail_status();
        }

        event_queue
            .dispatch_pending(&mut state)
            .map_err(|e| format!("dispatch events: {e}"))?;

        state.tick_status();
        state.redraw();

        let _ = conn.flush();
        let sleep_ms = if state.visible {
            LOOP_SLEEP_VISIBLE_MS
        } else {
            LOOP_SLEEP_HIDDEN_MS
        };
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }
}

struct AppState {
    compositor: wl_compositor::WlCompositor,
    layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    shm: wl_shm::WlShm,
    qh: QueueHandle<Self>,
    monitors: Vec<MonitorBar>,
    visible: bool,
    detail_mode: bool,
    last_workspace_poll: Instant,
    last_volume_poll: Instant,
    last_song_poll: Instant,
    last_battery_refresh: Instant,
    last_datetime_refresh: Instant,
    status_events: Receiver<StatusEvent>,
    workspace_event_driven: bool,
    status: BarStatus,
}

struct MonitorBar {
    global_name: u32,
    _output: wl_output::WlOutput,
    output_name: Option<String>,
    surface: wl_surface::WlSurface,
    layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    width: u32,
    height: u32,
    configured: bool,
    closed: bool,
    needs_redraw: bool,
    visible_buffer_dirty: bool,
    hidden_buffer_dirty: bool,
    visible_buffer: Option<ShmBarBuffer>,
    hidden_buffer: Option<ShmBarBuffer>,
}

impl AppState {
    fn new(
        compositor: wl_compositor::WlCompositor,
        layer_shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
        shm: wl_shm::WlShm,
        qh: QueueHandle<Self>,
        streams: StatusEventStreams,
    ) -> Self {
        Self {
            compositor,
            layer_shell,
            shm,
            qh,
            monitors: Vec::new(),
            visible: false,
            detail_mode: false,
            last_workspace_poll: Instant::now() - Duration::from_millis(WORKSPACE_POLL_HIDDEN_MS),
            last_volume_poll: Instant::now() - Duration::from_millis(VOLUME_POLL_HIDDEN_MS),
            last_song_poll: Instant::now() - Duration::from_millis(SONG_POLL_HIDDEN_MS),
            last_battery_refresh: Instant::now() - Duration::from_millis(BATTERY_REFRESH_MS),
            last_datetime_refresh: Instant::now()
                - Duration::from_millis(DATETIME_REFRESH_HIDDEN_MS),
            status_events: streams.rx,
            workspace_event_driven: streams.workspace_event_driven,
            status: BarStatus::gather(false),
        }
    }

    fn add_output(&mut self, registry: &wl_registry::WlRegistry, global_name: u32, version: u32) {
        if self
            .monitors
            .iter()
            .any(|monitor| monitor.global_name == global_name)
        {
            return;
        }

        let output = registry.bind::<wl_output::WlOutput, _, _>(
            global_name,
            version.min(4),
            &self.qh,
            global_name,
        );
        let surface = self.compositor.create_surface(&self.qh, ());
        let layer_surface = self.layer_shell.get_layer_surface(
            &surface,
            Some(&output),
            zwlr_layer_shell_v1::Layer::Overlay,
            "disturbar".to_string(),
            &self.qh,
            global_name,
        );

        layer_surface.set_anchor(
            zwlr_layer_surface_v1::Anchor::Top
                | zwlr_layer_surface_v1::Anchor::Left
                | zwlr_layer_surface_v1::Anchor::Right,
        );
        layer_surface.set_margin(MARGIN_TOP, MARGIN_SIDE, 0, MARGIN_SIDE);
        layer_surface
            .set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
        layer_surface.set_exclusive_zone(0);
        layer_surface.set_size(0, BAR_HEIGHT);

        let empty_input_region = self.compositor.create_region(&self.qh, ());
        surface.set_input_region(Some(&empty_input_region));
        empty_input_region.destroy();
        surface.commit();

        self.monitors.push(MonitorBar {
            global_name,
            _output: output,
            output_name: None,
            surface,
            layer_surface,
            width: 0,
            height: BAR_HEIGHT,
            configured: false,
            closed: false,
            needs_redraw: false,
            visible_buffer_dirty: true,
            hidden_buffer_dirty: true,
            visible_buffer: None,
            hidden_buffer: None,
        });
    }

    fn remove_output(&mut self, global_name: u32) {
        if let Some(idx) = self
            .monitors
            .iter()
            .position(|monitor| monitor.global_name == global_name)
        {
            let monitor = self.monitors.remove(idx);
            monitor.destroy();
        }
    }

    fn has_unconfigured_monitors(&self) -> bool {
        self.monitors
            .iter()
            .any(|monitor| !monitor.closed && !monitor.configured)
    }

    fn mark_all_for_redraw(&mut self) {
        for monitor in &mut self.monitors {
            if monitor.closed || !monitor.configured {
                continue;
            }
            monitor.needs_redraw = true;
        }
    }

    fn mark_all_visible_buffers_dirty(&mut self) {
        for monitor in &mut self.monitors {
            monitor.mark_visible_dirty(self.visible);
        }
    }

    fn refresh_detail_status(&mut self) {
        self.status.battery = status::gather_battery(self.detail_mode);
        self.status.volume = status::gather_volume(self.detail_mode);
        self.mark_all_visible_buffers_dirty();
    }

    fn tick_status(&mut self) {
        let now = Instant::now();
        let mut workspace_dirty = false;
        let workspace_poll_ms = if self.visible {
            WORKSPACE_POLL_VISIBLE_MS
        } else {
            WORKSPACE_POLL_HIDDEN_MS
        };
        let volume_poll_ms = if self.visible {
            VOLUME_POLL_VISIBLE_MS
        } else {
            VOLUME_POLL_HIDDEN_MS
        };
        let song_poll_ms = if self.visible {
            SONG_POLL_VISIBLE_MS
        } else {
            SONG_POLL_HIDDEN_MS
        };
        let datetime_refresh_ms = if self.visible {
            DATETIME_REFRESH_VISIBLE_MS
        } else {
            DATETIME_REFRESH_HIDDEN_MS
        };

        while let Ok(event) = self.status_events.try_recv() {
            match event {
                StatusEvent::WorkspaceDirty => workspace_dirty = true,
            }
        }

        if workspace_dirty {
            self.apply_workspace_update(BarStatus::gather_workspaces());
        }

        if !self.workspace_event_driven
            && now.duration_since(self.last_workspace_poll)
                >= Duration::from_millis(workspace_poll_ms)
        {
            self.apply_workspace_update(BarStatus::gather_workspaces());
            self.last_workspace_poll = now;
        }

        if now.duration_since(self.last_volume_poll) >= Duration::from_millis(volume_poll_ms) {
            let next_volume = status::gather_volume(self.detail_mode);
            if self.status.volume != next_volume {
                self.status.volume = next_volume;
                self.mark_all_visible_buffers_dirty();
            }
            self.last_volume_poll = now;
        }

        if now.duration_since(self.last_song_poll) >= Duration::from_millis(song_poll_ms) {
            let next_song = status::gather_song();
            if self.status.song != next_song {
                self.status.song = next_song;
                self.mark_all_visible_buffers_dirty();
            }
            self.last_song_poll = now;
        }

        if now.duration_since(self.last_battery_refresh)
            >= Duration::from_millis(BATTERY_REFRESH_MS)
        {
            let next_battery = status::gather_battery(self.detail_mode);
            if self.status.battery != next_battery {
                self.status.battery = next_battery;
                self.mark_all_visible_buffers_dirty();
            }
            self.last_battery_refresh = now;
        }

        if now.duration_since(self.last_datetime_refresh)
            >= Duration::from_millis(datetime_refresh_ms)
        {
            let next_datetime = status::gather_datetime();
            if self.status.datetime != next_datetime {
                self.status.datetime = next_datetime;
                self.mark_all_visible_buffers_dirty();
            }
            self.last_datetime_refresh = now;
        }
    }

    fn apply_workspace_update(&mut self, next: WorkspaceStatus) {
        if self.status.workspaces == next {
            return;
        }

        for monitor in &mut self.monitors {
            let old_label = self
                .status
                .workspaces
                .label_for_monitor(monitor.output_name.as_deref());
            let next_label = next.label_for_monitor(monitor.output_name.as_deref());
            if old_label != next_label {
                monitor.mark_visible_dirty(self.visible);
            }
        }

        self.status.workspaces = next;
    }

    fn redraw(&mut self) {
        let right = format!(
            "{}  {}  {}",
            self.status.battery, self.status.volume, self.status.datetime
        );

        for monitor in &mut self.monitors {
            monitor.redraw(
                &self.shm,
                &self.qh,
                self.visible,
                &self.status.workspaces,
                &self.status.song,
                &right,
            );
        }
    }
}

impl MonitorBar {
    fn mark_visible_dirty(&mut self, visible: bool) {
        self.visible_buffer_dirty = true;
        self.visible_buffer = None;
        if visible && self.configured && !self.closed {
            self.needs_redraw = true;
        }
    }

    fn set_output_name(&mut self, output_name: String, visible: bool) {
        if self.output_name.as_deref() == Some(output_name.as_str()) {
            return;
        }
        self.output_name = Some(output_name);
        self.mark_visible_dirty(visible);
    }

    fn configure(&mut self, width: u32, height: u32) {
        self.configured = true;
        self.width = width;
        self.height = if height == 0 { BAR_HEIGHT } else { height };
        self.visible_buffer_dirty = true;
        self.hidden_buffer_dirty = true;
        self.visible_buffer = None;
        self.hidden_buffer = None;
        self.needs_redraw = true;
    }

    fn ensure_visible_buffer(
        &mut self,
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<AppState>,
        workspaces: &WorkspaceStatus,
        center: &str,
        right: &str,
    ) {
        if !self.visible_buffer_dirty && self.visible_buffer.is_some() {
            return;
        }

        let left = workspaces.label_for_monitor(self.output_name.as_deref());
        let pixels = renderer::render_visible_pixels(
            self.width,
            self.height,
            BAR_VISIBLE_RGBA,
            TEXT_RGBA,
            left,
            center,
            right,
        );
        self.visible_buffer =
            renderer::create_buffer_from_pixels(shm, qh, self.width, self.height, &pixels);
        self.visible_buffer_dirty = false;
    }

    fn ensure_hidden_buffer(&mut self, shm: &wl_shm::WlShm, qh: &QueueHandle<AppState>) {
        if !self.hidden_buffer_dirty && self.hidden_buffer.is_some() {
            return;
        }

        self.hidden_buffer =
            renderer::create_solid_bar_buffer(shm, qh, self.width, self.height, BAR_HIDDEN_RGBA);
        self.hidden_buffer_dirty = false;
    }

    fn redraw(
        &mut self,
        shm: &wl_shm::WlShm,
        qh: &QueueHandle<AppState>,
        visible: bool,
        workspaces: &WorkspaceStatus,
        center: &str,
        right: &str,
    ) {
        if !self.needs_redraw
            || !self.configured
            || self.closed
            || self.width == 0
            || self.height == 0
        {
            return;
        }

        let buffer = if visible {
            self.ensure_visible_buffer(shm, qh, workspaces, center, right);
            self.visible_buffer.as_ref()
        } else {
            self.ensure_hidden_buffer(shm, qh);
            self.hidden_buffer.as_ref()
        };

        let Some(buffer) = buffer else {
            return;
        };

        self.surface.attach(Some(&buffer.buffer), 0, 0);
        self.surface
            .damage_buffer(0, 0, self.width as i32, self.height as i32);
        self.surface.commit();
        self.needs_redraw = false;
    }

    fn destroy(self) {
        self.layer_surface.destroy();
        self.surface.destroy();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for AppState {
    fn event(
        state: &mut Self,
        proxy: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name,
                interface,
                version,
            } => {
                if interface == "wl_output" {
                    state.add_output(proxy, name, version);
                }
            }
            wl_registry::Event::GlobalRemove { name } => state.remove_output(name),
            _ => {}
        }
    }
}

impl Dispatch<wl_output::WlOutput, u32> for AppState {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        global_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event
            && let Some(monitor) = state
                .monitors
                .iter_mut()
                .find(|monitor| monitor.global_name == *global_name)
        {
            monitor.set_output_name(name, state.visible);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, u32> for AppState {
    fn event(
        state: &mut Self,
        proxy: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        global_name: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(monitor) = state
            .monitors
            .iter_mut()
            .find(|monitor| monitor.global_name == *global_name)
        else {
            return;
        };

        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                proxy.ack_configure(serial);
                monitor.configure(width, height);
            }
            zwlr_layer_surface_v1::Event::Closed => {
                monitor.closed = true;
            }
            _ => {}
        }
    }
}

delegate_noop!(AppState: ignore wl_buffer::WlBuffer);
delegate_noop!(AppState: ignore wl_compositor::WlCompositor);
delegate_noop!(AppState: ignore wl_region::WlRegion);
delegate_noop!(AppState: ignore wl_shm::WlShm);
delegate_noop!(AppState: ignore wl_shm_pool::WlShmPool);
delegate_noop!(AppState: ignore wl_surface::WlSurface);
delegate_noop!(AppState: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
