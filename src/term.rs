use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::OnceLock;
use std::time::Duration;

const ENTER: &str = "\x1b[?1049h\x1b[?25l\x1b[2J";
const LEAVE: &str = "\x1b[0m\x1b[?25h\x1b[?1049l";

static ORIGINAL: OnceLock<libc::termios> = OnceLock::new();

pub enum Key {
    Quit,
    Other,
}

pub struct Tty {
    tty: File,
}

impl Tty {
    pub fn open() -> io::Result<Tty> {
        let tty = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let fd = tty.as_raw_fd();
        let mut t = unsafe { std::mem::zeroed::<libc::termios>() };
        if unsafe { libc::tcgetattr(fd, &mut t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let _ = ORIGINAL.set(t);
        unsafe { libc::cfmakeraw(&mut t) };
        if unsafe { libc::tcsetattr(fd, libc::TCSAFLUSH, &t) } != 0 {
            return Err(io::Error::last_os_error());
        }

        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore();
            hook(info);
        }));

        let mut out = io::stdout();
        out.write_all(ENTER.as_bytes())?;
        out.flush()?;
        Ok(Tty { tty })
    }

    pub fn size(&self) -> (u16, u16) {
        let mut ws = unsafe { std::mem::zeroed::<libc::winsize>() };
        if unsafe { libc::ioctl(self.tty.as_raw_fd(), libc::TIOCGWINSZ, &mut ws) } != 0 {
            return (80, 24);
        }
        (ws.ws_col, ws.ws_row)
    }

    pub fn key(&mut self, timeout: Duration) -> io::Result<Option<Key>> {
        let fd = self.tty.as_raw_fd();
        let mut fds = unsafe { std::mem::zeroed::<libc::fd_set>() };
        unsafe { libc::FD_SET(fd, &mut fds) };
        let mut tv = libc::timeval {
            tv_sec: 0,
            tv_usec: timeout.as_micros() as libc::suseconds_t,
        };
        let ready = unsafe {
            libc::select(
                fd + 1,
                &mut fds,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut tv,
            )
        };
        if ready < 0 {
            let err = io::Error::last_os_error();
            return match err.kind() {
                io::ErrorKind::Interrupted => Ok(None),
                _ => Err(err),
            };
        }
        if ready == 0 {
            return Ok(None);
        }
        let mut buf = [0u8; 64];
        let n = self.tty.read(&mut buf)?;
        let quit = matches!(&buf[..n], [0x1b]) || buf[..n].iter().any(|&b| b == b'q' || b == 0x03);
        Ok(Some(if quit { Key::Quit } else { Key::Other }))
    }
}

impl Drop for Tty {
    fn drop(&mut self) {
        restore();
    }
}

fn restore() {
    let mut out = io::stdout();
    let _ = out.write_all(LEAVE.as_bytes());
    let _ = out.flush();
    if let (Some(t), Ok(tty)) = (ORIGINAL.get(), File::open("/dev/tty")) {
        unsafe { libc::tcsetattr(tty.as_raw_fd(), libc::TCSAFLUSH, t) };
    }
}
