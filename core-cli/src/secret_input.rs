use bili_core::{CoreError, ErrorCode, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use std::{
    io::{self, Read},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

const MAX_COOKIE_BYTES: usize = 65_536;
const EVENT_POLL_INTERVAL: Duration = Duration::from_millis(50);

pub(crate) async fn prompt_cookie(prompt: &'static str) -> Result<String> {
    let mut terminal = TerminalMode::enable()?;
    let cancelled = Arc::clone(&terminal.cancelled);
    let worker = tokio::task::spawn_blocking(move || {
        let reader = TerminalReader::new(cancelled);
        let config = rpassword::ConfigBuilder::new()
            .input_reader(reader)
            .output_writer(io::stderr())
            .build();
        rpassword::prompt_password_with_config(prompt, config).map(Zeroizing::new)
    });

    let password = worker
        .await
        .map_err(|_| CoreError::new(ErrorCode::Corruption, "secret input worker failed"))?;
    let restore = terminal.restore();
    let mut password = match password {
        Ok(password) => password,
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::Interrupted | io::ErrorKind::ConnectionAborted
            ) =>
        {
            return Err(CoreError::new(
                ErrorCode::Cancelled,
                "secret input cancelled",
            ));
        }
        Err(_) => return Err(crate::invalid("read hidden Cookie input")),
    };
    restore.map_err(|_| CoreError::new(ErrorCode::Filesystem, "restore terminal mode"))?;
    if password.len() > MAX_COOKIE_BYTES {
        return Err(crate::invalid("Cookie input exceeds 64 KiB"));
    }
    Ok(std::mem::take(&mut *password))
}

struct TerminalMode {
    cancelled: Arc<AtomicBool>,
    enabled: bool,
}

impl TerminalMode {
    fn enable() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()
            .map_err(|_| CoreError::new(ErrorCode::Filesystem, "enable hidden terminal input"))?;
        Ok(Self {
            cancelled: Arc::new(AtomicBool::new(false)),
            enabled: true,
        })
    }

    fn restore(&mut self) -> io::Result<()> {
        self.cancelled.store(true, Ordering::Release);
        if self.enabled {
            crossterm::terminal::disable_raw_mode()?;
            self.enabled = false;
        }
        Ok(())
    }
}

impl Drop for TerminalMode {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        if self.enabled {
            self.enabled = false;
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}

struct TerminalReader {
    cancelled: Arc<AtomicBool>,
    pending: [u8; 4],
    pending_len: usize,
    pending_offset: usize,
    input_bytes: usize,
}

impl TerminalReader {
    fn new(cancelled: Arc<AtomicBool>) -> Self {
        Self {
            cancelled,
            pending: [0; 4],
            pending_len: 0,
            pending_offset: 0,
            input_bytes: 0,
        }
    }

    fn next_bytes(&mut self) -> io::Result<()> {
        loop {
            if self.cancelled.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "secret input cancelled",
                ));
            }
            if !event::poll(EVENT_POLL_INTERVAL)? {
                continue;
            }
            if self.cancelled.load(Ordering::Acquire) {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "secret input cancelled",
                ));
            }
            let Event::Key(key) = event::read()? else {
                continue;
            };
            if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
                continue;
            }
            let Some((bytes, len)) = key_bytes(key) else {
                continue;
            };
            if bytes[0] == b'\n' || bytes[0] == b'\r' {
                self.pending = bytes;
                self.pending_len = len;
                self.pending_offset = 0;
                return Ok(());
            }
            if bytes[0] != 0x03 {
                let Some(total) = bounded_input_len(self.input_bytes, len) else {
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "secret input exceeds limit",
                    ));
                };
                self.input_bytes = total;
            }
            self.pending = bytes;
            self.pending_len = len;
            self.pending_offset = 0;
            return Ok(());
        }
    }
}

impl Read for TerminalReader {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        if output.is_empty() {
            return Ok(0);
        }
        if self.pending_offset == self.pending_len {
            self.next_bytes()?;
        }
        let count = output.len().min(self.pending_len - self.pending_offset);
        output[..count]
            .copy_from_slice(&self.pending[self.pending_offset..self.pending_offset + count]);
        self.pending_offset += count;
        if self.pending_offset == self.pending_len {
            self.pending.zeroize();
            self.pending_len = 0;
            self.pending_offset = 0;
        }
        Ok(count)
    }
}

impl Drop for TerminalReader {
    fn drop(&mut self) {
        self.pending.zeroize();
    }
}

fn key_bytes(key: KeyEvent) -> Option<([u8; 4], usize)> {
    let mut bytes = [0; 4];
    let len = match key.code {
        KeyCode::Enter => {
            bytes[0] = b'\n';
            1
        }
        KeyCode::Backspace => {
            bytes[0] = 0x08;
            1
        }
        KeyCode::Char(ch) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            bytes[0] = match ch.to_ascii_lowercase() {
                'c' => 0x03,
                'd' => 0x04,
                'j' => b'\n',
                'm' => b'\r',
                'u' => 0x15,
                'w' => 0x17,
                _ => return None,
            };
            1
        }
        KeyCode::Char(ch) if !ch.is_control() => ch.encode_utf8(&mut bytes).len(),
        _ => return None,
    };
    Some((bytes, len))
}

fn bounded_input_len(current: usize, additional: usize) -> Option<usize> {
    current
        .checked_add(additional)
        .filter(|total| *total <= MAX_COOKIE_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    #[test]
    fn maps_password_editing_keys_to_rpassword_control_bytes() {
        assert_eq!(
            key_bytes(key(KeyCode::Enter, KeyModifiers::NONE))
                .unwrap()
                .0[0],
            b'\n'
        );
        assert_eq!(
            key_bytes(key(KeyCode::Backspace, KeyModifiers::NONE))
                .unwrap()
                .0[0],
            0x08
        );
        assert_eq!(
            key_bytes(key(KeyCode::Char('j'), KeyModifiers::CONTROL))
                .unwrap()
                .0[0],
            b'\n'
        );
        assert_eq!(
            key_bytes(key(KeyCode::Char('m'), KeyModifiers::CONTROL))
                .unwrap()
                .0[0],
            b'\r'
        );
        for (ch, byte) in [('c', 0x03), ('d', 0x04), ('u', 0x15), ('w', 0x17)] {
            assert_eq!(
                key_bytes(key(KeyCode::Char(ch), KeyModifiers::CONTROL))
                    .unwrap()
                    .0[0],
                byte
            );
        }
        assert!(key_bytes(key(KeyCode::Left, KeyModifiers::NONE)).is_none());
        assert!(key_bytes(key(KeyCode::Char('x'), KeyModifiers::CONTROL)).is_none());
    }

    #[test]
    fn bounds_typed_bytes_and_prioritizes_cancellation() {
        assert_eq!(
            bounded_input_len(MAX_COOKIE_BYTES - 1, 1),
            Some(MAX_COOKIE_BYTES)
        );
        assert_eq!(bounded_input_len(MAX_COOKIE_BYTES, 1), None);

        let cancelled = Arc::new(AtomicBool::new(true));
        let mut reader = TerminalReader::new(cancelled);
        let error = reader.read(&mut [0]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
    }

    #[test]
    fn maps_unicode_to_utf8_without_dynamic_pending_storage() {
        let (bytes, len) = key_bytes(key(KeyCode::Char('界'), KeyModifiers::NONE)).unwrap();
        assert_eq!(&bytes[..len], "界".as_bytes());
        assert_eq!(len, 3);
    }
}
