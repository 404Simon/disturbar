use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use wayland_client::globals::{GlobalListContents, registry_queue_init};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::constants::{
    BAR_HEIGHT, BAR_HIDDEN_RGBA, BAR_VISIBLE_RGBA, BATTERY_REFRESH_MS, DATETIME_REFRESH_HIDDEN_MS,
    DATETIME_REFRESH_VISIBLE_MS, LOOP_SLEEP_HIDDEN_MS, LOOP_SLEEP_VISIBLE_MS, MARGIN_SIDE,
    MARGIN_TOP, SIGNAL_HIDE, SIGNAL_SHOW, SONG_POLL_HIDDEN_MS, SONG_POLL_VISIBLE_MS, TEXT_RGBA,
    VOLUME_POLL_HIDDEN_MS, VOLUME_POLL_VISIBLE_MS, WORKSPACE_POLL_HIDDEN_MS,
    WORKSPACE_POLL_VISIBLE_MS, SIGNAL_DETAIL_OFF, SIGNAL_DETAIL_ON,
};
use crate::renderer::{self, ShmBarBuffer};
use crate::signals;
use crate::status::{self, BarStatus, StatusEvent, StatusEventStreams};

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

    let surface = compositor.create_surface(&qh, ());
    let layer_surface = layer_shell.get_layer_surface(
        &surface,
        None,
        zwlr_layer_shell_v1::Layer::Overlay,
        "disturbar".to_string(),
        &qh,
        (),
    );

    layer_surface.set_anchor(
        zwlr_layer_surface_v1::Anchor::Top
            | zwlr_layer_surface_v1::Anchor::Left
            | zwlr_layer_surface_v1::Anchor::Right,
    );
    layer_surface.set_margin(MARGIN_TOP, MARGIN_SIDE, 0, MARGIN_SIDE);
    layer_surface.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);
    layer_surface.set_exclusive_zone(0);
    layer_surface.set_size(0, BAR_HEIGHT);
    let empty_input_region = compositor.create_region(&qh, ());
    surface.set_input_region(Some(&empty_input_region));
    empty_input_region.destroy();

    surface.commit();

    let streams = status::spawn_status_event_streams();
    let mut state = AppState::new(surface, layer_surface, shm, qh, streams);

    while !state.configured && !state.closed {
        event_queue
            .blocking_dispatch(&mut state)
            .map_err(|e| format!("dispatch init events: {e}"))?;
    }

    if state.needs_redraw {
        if state.visible {
            state.tick_status();
        }
        state.redraw();
        state.needs_redraw = false;
    }

    loop {
        let signal = signals::take_visibility_signal();
        if signal & SIGNAL_SHOW != 0 && !state.visible {
            state.visible = true;
            state.needs_redraw = true;
        }
        if signal & SIGNAL_HIDE != 0 && state.visible {
            state.visible = false;
            state.needs_redraw = true;
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

        if state.closed {
            break;
        }

        state.tick_status();

        if state.needs_redraw {
            state.redraw();
            state.needs_redraw = false;
        }

        let _ = conn.flush();
        let sleep_ms = if state.visible {
            LOOP_SLEEP_VISIBLE_MS
        } else {
            LOOP_SLEEP_HIDDEN_MS
        };
        std::thread::sleep(Duration::from_millis(sleep_ms));
    }

    Ok(())
}

struct AppState {
    surface: wl_surface::WlSurface,
    _layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
    shm: wl_shm::WlShm,
    qh: QueueHandle<Self>,
    width: u32,
    height: u32,
    configured: bool,
    closed: bool,
    visible: bool,
    needs_redraw: bool,
    detail_mode: bool,
    last_workspace_poll: Instant,
    last_volume_poll: Instant,
    last_song_poll: Instant,
    last_battery_refresh: Instant,
    last_datetime_refresh: Instant,
    status_events: Receiver<StatusEvent>,
    workspace_event_driven: bool,
    status: BarStatus,
    visible_buffer: Option<ShmBarBuffer>,
    hidden_buffer: Option<ShmBarBuffer>,
}

impl AppState {
    fn new(
        surface: wl_surface::WlSurface,
        layer_surface: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        shm: wl_shm::WlShm,
        qh: QueueHandle<Self>,
        streams: StatusEventStreams,
    ) -> Self {
        Self {
            surface,
            _layer_surface: layer_surface,
            shm,
            qh,
            width: 0,
            height: BAR_HEIGHT,
            configured: false,
            closed: false,
            visible: false,
            needs_redraw: false,
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
            visible_buffer: None,
            hidden_buffer: None,
        }
    }

    fn refresh_detail_status(&mut self) {
        self.status.battery = status::gather_battery(self.detail_mode);
        self.status.volume = status::gather_volume(self.detail_mode);
        self.recreate_buffers();
        if self.visible {
            self.needs_redraw = true;
        }
    }

    fn tick_status(&mut self) {
        let now = Instant::now();
        let mut changed = false;
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
            let next_workspaces = BarStatus::gather_workspaces();
            if self.status.workspaces != next_workspaces {
                self.status.workspaces = next_workspaces;
                changed = true;
            }
        }

        if !self.workspace_event_driven
            && now.duration_since(self.last_workspace_poll)
                >= Duration::from_millis(workspace_poll_ms)
        {
            let next_workspaces = BarStatus::gather_workspaces();
            if self.status.workspaces != next_workspaces {
                self.status.workspaces = next_workspaces;
                changed = true;
            }
            self.last_workspace_poll = now;
        }

        if now.duration_since(self.last_volume_poll) >= Duration::from_millis(volume_poll_ms) {
            let next_volume = status::gather_volume(self.detail_mode);
            if self.status.volume != next_volume {
                self.status.volume = next_volume;
                changed = true;
            }
            self.last_volume_poll = now;
        }

        if now.duration_since(self.last_song_poll) >= Duration::from_millis(song_poll_ms) {
            let next_song = status::gather_song();
            if self.status.song != next_song {
                self.status.song = next_song;
                changed = true;
            }
            self.last_song_poll = now;
        }

        if now.duration_since(self.last_battery_refresh)
            >= Duration::from_millis(BATTERY_REFRESH_MS)
        {
            let next_battery = status::gather_battery(self.detail_mode);
            if self.status.battery != next_battery {
                self.status.battery = next_battery;
                changed = true;
            }
            self.last_battery_refresh = now;
        }

        if now.duration_since(self.last_datetime_refresh)
            >= Duration::from_millis(datetime_refresh_ms)
        {
            let next_datetime = status::gather_datetime();
            if self.status.datetime != next_datetime {
                self.status.datetime = next_datetime;
                changed = true;
            }
            self.last_datetime_refresh = now;
        }

        if changed {
            self.recreate_buffers();
            if self.visible {
                self.needs_redraw = true;
            }
        }
    }

    fn redraw(&mut self) {
        if !self.configured || self.width == 0 || self.height == 0 {
            return;
        }

        self.ensure_buffers();

        let selected = if self.visible {
            self.visible_buffer.as_ref()
        } else {
            self.hidden_buffer.as_ref()
        };

        let Some(selected) = selected else {
            return;
        };

        self.surface.attach(Some(&selected.buffer), 0, 0);
        self.surface
            .damage_buffer(0, 0, self.width as i32, self.height as i32);
        self.surface.commit();
    }

    fn ensure_buffers(&mut self) {
        if self.visible_buffer.is_some() {
            return;
        }

        let right = format!(
            "{}  {}  {}",
            self.status.battery, self.status.volume, self.status.datetime
        );
        let visible_pixels = renderer::render_visible_pixels(
            self.width,
            self.height,
            BAR_VISIBLE_RGBA,
            TEXT_RGBA,
            &self.status.workspaces,
            &self.status.song,
            &right,
        );
        self.visible_buffer = renderer::create_buffer_from_pixels(
            &self.shm,
            &self.qh,
            self.width,
            self.height,
            &visible_pixels,
        );
        self.hidden_buffer = renderer::create_solid_bar_buffer(
            &self.shm,
            &self.qh,
            self.width,
            self.height,
            BAR_HIDDEN_RGBA,
        );
    }

    fn recreate_buffers(&mut self) {
        self.visible_buffer = None;
        self.hidden_buffer = None;
        self.ensure_buffers();
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for AppState {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for AppState {
    fn event(
        state: &mut Self,
        proxy: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure {
                serial,
                width,
                height,
            } => {
                proxy.ack_configure(serial);
                state.configured = true;
                state.width = width;
                state.height = if height == 0 { BAR_HEIGHT } else { height };
                state.recreate_buffers();
                state.needs_redraw = true;
            }
            zwlr_layer_surface_v1::Event::Closed => {
                state.closed = true;
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
