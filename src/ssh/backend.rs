use ratatui::{backend::WindowSize, buffer::Cell, prelude::*};
use std::io::{Result, Write};

use crate::ssh::app::TerminalHandle;

pub struct SshBackend {
    handle: TerminalHandle,
    width: u32,
    height: u32,
    cursor_pos: Position,
}

impl SshBackend {
    pub fn new(handle: TerminalHandle) -> Self {
        Self {
            handle,
            width: 160,
            height: 48,
            cursor_pos: Position { x: 0, y: 0 },
        }
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
    }

    fn write_color(&mut self, color: Color, is_bg: bool) -> Result<()> {
        let prefix = if is_bg { 48 } else { 38 };

        match color {
            Color::Reset => write!(self.handle, "\x1b[{}m", if is_bg { 49 } else { 39 }),
            Color::Black => write!(self.handle, "\x1b[{}m", if is_bg { 40 } else { 30 }),
            Color::Red => write!(self.handle, "\x1b[{}m", if is_bg { 41 } else { 31 }),
            Color::Green => write!(self.handle, "\x1b[{}m", if is_bg { 42 } else { 32 }),
            Color::Yellow => write!(self.handle, "\x1b[{}m", if is_bg { 43 } else { 33 }),
            Color::Blue => write!(self.handle, "\x1b[{}m", if is_bg { 44 } else { 34 }),
            Color::Magenta => write!(self.handle, "\x1b[{}m", if is_bg { 45 } else { 35 }),
            Color::Cyan => write!(self.handle, "\x1b[{}m", if is_bg { 46 } else { 36 }),
            Color::Gray | Color::White => {
                write!(self.handle, "\x1b[{}m", if is_bg { 47 } else { 37 })
            }
            Color::DarkGray => write!(self.handle, "\x1b[{}m", if is_bg { 100 } else { 90 }),
            Color::LightRed => write!(self.handle, "\x1b[{}m", if is_bg { 101 } else { 91 }),
            Color::LightGreen => write!(self.handle, "\x1b[{}m", if is_bg { 102 } else { 92 }),
            Color::LightYellow => write!(self.handle, "\x1b[{}m", if is_bg { 103 } else { 93 }),
            Color::LightBlue => write!(self.handle, "\x1b[{}m", if is_bg { 104 } else { 94 }),
            Color::LightMagenta => write!(self.handle, "\x1b[{}m", if is_bg { 105 } else { 95 }),
            Color::LightCyan => write!(self.handle, "\x1b[{}m", if is_bg { 106 } else { 96 }),
            Color::Rgb(r, g, b) => write!(self.handle, "\x1b[{prefix};2;{r};{g};{b}m"),
            Color::Indexed(i) => write!(self.handle, "\x1b[{prefix};5;{i}m"),
        }
    }

    fn write_modifiers(&mut self, modifier: Modifier) -> Result<()> {
        if modifier.contains(Modifier::BOLD) {
            write!(self.handle, "\x1b[1m")?;
        }
        if modifier.contains(Modifier::DIM) {
            write!(self.handle, "\x1b[2m")?;
        }
        if modifier.contains(Modifier::ITALIC) {
            write!(self.handle, "\x1b[3m")?;
        }
        if modifier.contains(Modifier::UNDERLINED) {
            write!(self.handle, "\x1b[4m")?;
        }
        if modifier.contains(Modifier::SLOW_BLINK) || modifier.contains(Modifier::RAPID_BLINK) {
            write!(self.handle, "\x1b[5m")?;
        }
        if modifier.contains(Modifier::REVERSED) {
            write!(self.handle, "\x1b[7m")?;
        }
        if modifier.contains(Modifier::HIDDEN) {
            write!(self.handle, "\x1b[8m")?;
        }
        if modifier.contains(Modifier::CROSSED_OUT) {
            write!(self.handle, "\x1b[9m")?;
        }
        Ok(())
    }
}

impl Backend for SshBackend {
    fn draw<'a, I>(&mut self, content: I) -> Result<()>
    where
        I: Iterator<Item = (u16, u16, &'a Cell)>,
    {
        let mut last_pos: Option<(u16, u16)> = None;
        let mut last_style: Option<Style> = None;

        for (x, y, cell) in content {
            // Move cursor only if necessary
            if last_pos != Some((x, y)) {
                write!(self.handle, "\x1b[{};{}H", y + 1, x + 1)?;
            }

            // Update style only if changed
            let style = cell.style();
            if last_style.as_ref() != Some(&style) {
                // Reset all attributes
                write!(self.handle, "\x1b[0m")?;

                // Set foreground color
                if let Some(c) = style.fg
                    && c != Color::Reset
                {
                    self.write_color(c, false)?;
                }

                // Set background color
                if let Some(c) = style.bg
                    && c != Color::Reset
                {
                    self.write_color(c, true)?;
                }

                // Set underline color if present
                if let Some(color) = style.underline_color {
                    write!(
                        self.handle,
                        "\x1b[58;2;{};{};{}m",
                        match color {
                            Color::Rgb(r, g, b) => (r, g, b),
                            _ => (255, 255, 255), // fallback
                        }
                        .0,
                        match color {
                            Color::Rgb(r, g, b) => (r, g, b),
                            _ => (255, 255, 255),
                        }
                        .1,
                        match color {
                            Color::Rgb(r, g, b) => (r, g, b),
                            _ => (255, 255, 255),
                        }
                        .2
                    )?;
                }

                // Set modifiers
                self.write_modifiers(style.add_modifier)?;

                last_style = Some(style);
            }

            // Write the character
            write!(self.handle, "{}", cell.symbol())?;

            // Update expected cursor position
            last_pos = Some((x + 1, y));
        }

        // Reset style at the end
        write!(self.handle, "\x1b[0m")?;
        Ok(())
    }

    fn hide_cursor(&mut self) -> Result<()> {
        write!(self.handle, "\x1b[?25l")?;
        self.flush()
    }

    fn show_cursor(&mut self) -> Result<()> {
        write!(self.handle, "\x1b[?25h")?;
        self.flush()
    }

    fn get_cursor_position(&mut self) -> Result<Position> {
        Ok(self.cursor_pos)
    }

    fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> Result<()> {
        let pos = position.into();
        self.cursor_pos = pos;
        write!(self.handle, "\x1b[{};{}H", pos.y + 1, pos.x + 1)?;
        self.flush()
    }

    fn clear(&mut self) -> Result<()> {
        write!(self.handle, "\x1b[2J\x1b[H")?;
        self.flush()
    }

    fn size(&self) -> Result<Size> {
        Ok(Size {
            width: self.width as u16,
            height: self.height as u16,
        })
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
        self.handle.flush()
    }
}
