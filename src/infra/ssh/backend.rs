use ratatui::{backend::WindowSize, buffer::Cell, prelude::*};
use std::io::Result;

use super::app::TerminalHandle;

pub struct SshBackend {
    inner: CrosstermBackend<TerminalHandle>,
    size: Size,
}

impl SshBackend {
    pub fn new(handle: TerminalHandle) -> Self {
        Self {
            inner: CrosstermBackend::new(handle),
            size: Size {
                width: 160,
                height: 48,
            },
        }
    }

    pub fn resize(&mut self, width: u16, height: u16) {
        self.size = Size { width, height };
    }
}

impl Backend for SshBackend {
    fn draw<'a, I>(&mut self, content: I) -> Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        self.inner.draw(content)
    }

    fn hide_cursor(&mut self) -> Result<()> {
        self.inner.hide_cursor()
    }

    fn show_cursor(&mut self) -> Result<()> {
        self.inner.show_cursor()
    }

    fn get_cursor_position(&mut self) -> Result<Position> {
        self.inner.get_cursor_position()
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<()> {
        self.inner.set_cursor_position(position)
    }

    fn clear(&mut self) -> Result<()> {
        self.inner.clear()
    }

    fn size(&self) -> Result<Size> {
        Ok(self.size)
    }

    fn window_size(&mut self) -> Result<WindowSize> {
        Ok(WindowSize {
            columns_rows: self.size()?,
            pixels: Size {
                width: 0,
                height: 0,
            },
        })
    }

    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}
