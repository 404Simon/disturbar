use std::sync::atomic::{AtomicU8, Ordering};

use crate::constants::{SIGNAL_HIDE, SIGNAL_NONE, SIGNAL_SHOW};

static VISIBILITY_SIGNAL: AtomicU8 = AtomicU8::new(SIGNAL_NONE);

pub fn register_signal_handlers() {
    unsafe {
        let mut action_show: libc::sigaction = std::mem::zeroed();
        action_show.sa_sigaction = signal_show_handler as *const () as usize;
        action_show.sa_flags = 0;
        libc::sigemptyset(&mut action_show.sa_mask);
        libc::sigaction(libc::SIGUSR1, &action_show, std::ptr::null_mut());

        let mut action_hide: libc::sigaction = std::mem::zeroed();
        action_hide.sa_sigaction = signal_hide_handler as *const () as usize;
        action_hide.sa_flags = 0;
        libc::sigemptyset(&mut action_hide.sa_mask);
        libc::sigaction(libc::SIGUSR2, &action_hide, std::ptr::null_mut());
    }
}

pub fn take_visibility_signal() -> u8 {
    VISIBILITY_SIGNAL.swap(SIGNAL_NONE, Ordering::Relaxed)
}

extern "C" fn signal_show_handler(_: i32) {
    VISIBILITY_SIGNAL.store(SIGNAL_SHOW, Ordering::Relaxed);
}

extern "C" fn signal_hide_handler(_: i32) {
    VISIBILITY_SIGNAL.store(SIGNAL_HIDE, Ordering::Relaxed);
}
