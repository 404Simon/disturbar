pub const BAR_HEIGHT: u32 = 24;
pub const MARGIN_TOP: i32 = 16;
pub const MARGIN_SIDE: i32 = 20;

pub const BAR_VISIBLE_RGBA: [u8; 4] = [28, 28, 28, 230];
pub const BAR_HIDDEN_RGBA: [u8; 4] = [0, 0, 0, 0];
pub const TEXT_RGBA: [u8; 4] = [235, 235, 235, 255];

pub const WORKSPACE_POLL_VISIBLE_MS: u64 = 120;
pub const WORKSPACE_POLL_HIDDEN_MS: u64 = 1500;
pub const VOLUME_POLL_VISIBLE_MS: u64 = 300;
pub const VOLUME_POLL_HIDDEN_MS: u64 = 5000;
pub const SONG_POLL_VISIBLE_MS: u64 = 1000;
pub const SONG_POLL_HIDDEN_MS: u64 = 5000;
pub const BATTERY_REFRESH_MS: u64 = 15000;
pub const DATETIME_REFRESH_VISIBLE_MS: u64 = 1000;
pub const DATETIME_REFRESH_HIDDEN_MS: u64 = 60000;
pub const LOOP_SLEEP_VISIBLE_MS: u64 = 8;
pub const LOOP_SLEEP_HIDDEN_MS: u64 = 40;

pub const SIGNAL_NONE: u8 = 0;
pub const SIGNAL_SHOW: u8 = 1;
pub const SIGNAL_HIDE: u8 = 2;
pub const SIGNAL_DETAIL_ON: u8 = 4;
pub const SIGNAL_DETAIL_OFF: u8 = 8;
