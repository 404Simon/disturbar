use evdev_rs::enums::{EV_KEY, int_to_ev_key};
use input::event::KeyboardEvent;
use input::event::keyboard::KeyboardEventTrait;
use input::event::tablet_pad::KeyState;
use input::{Event, Libinput, LibinputInterface};
use nix::poll::{PollFd, PollFlags, poll};
use std::fs::{File, OpenOptions};
use std::os::fd::{AsRawFd, BorrowedFd};
use std::os::unix::{fs::OpenOptionsExt, io::OwnedFd};
use std::path::Path;

use crate::process;

struct Interface;

impl LibinputInterface for Interface {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        OpenOptions::new()
            .custom_flags(flags)
            .read(flags & libc::O_WRONLY == 0)
            .write(flags & libc::O_WRONLY != 0 || flags & libc::O_RDWR != 0)
            .open(path)
            .map(|file| file.into())
            .map_err(|err| err.raw_os_error().unwrap_or(libc::EACCES))
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(File::from(fd));
    }
}

pub fn run_input_backend() {
    let mut input = Libinput::new_with_udev(Interface);
    if input.udev_assign_seat("seat0").is_err() {
        eprintln!("disturbar: could not assign libinput seat0");
        return;
    }

    let fd = input.as_raw_fd();
    if fd == -1 {
        eprintln!("disturbar: libinput returned invalid file descriptor");
        return;
    }

    let borrowed_fd = unsafe { BorrowedFd::borrow_raw(fd) };
    let pollfd = PollFd::new(borrowed_fd, PollFlags::POLLIN);

    let mut left_meta = false;
    let mut right_meta = false;
    let mut left_shift = false;
    let mut right_shift = false;
    let mut visible = false;
    let mut detail = false;

    while poll(&mut [pollfd.clone()], None::<u8>).is_ok() {
        if let Err(err) = input.dispatch() {
            eprintln!("disturbar: libinput dispatch failed: {err}");
            continue;
        }

        for event in &mut input {
            let event = match event {
                Event::Keyboard(KeyboardEvent::Key(event)) => event,
                _ => continue,
            };

            let Some(ev_key) = int_to_ev_key(event.key()) else {
                continue;
            };

            let pressed = event.key_state() == KeyState::Pressed;

            match ev_key {
                EV_KEY::KEY_LEFTMETA => left_meta = pressed,
                EV_KEY::KEY_RIGHTMETA => right_meta = pressed,
                EV_KEY::KEY_LEFTSHIFT => left_shift = pressed,
                EV_KEY::KEY_RIGHTSHIFT => right_shift = pressed,
                _ => continue,
            }

            let next_visible = left_meta || right_meta;
            if next_visible != visible {
                process::send_signal(if next_visible {
                    libc::SIGUSR1
                } else {
                    libc::SIGUSR2
                });
                visible = next_visible;
            }

            let next_detail = next_visible && (left_shift || right_shift);
            if next_detail != detail {
                process::send_signal(if next_detail {
                    libc::SIGWINCH
                } else {
                    libc::SIGURG
                });
                detail = next_detail;
            }
        }
    }

    eprintln!("disturbar: input backend stopped");
}
