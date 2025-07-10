use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    vec::Vec,
};
use bootloader::gop::GopInfo;

use crate::screen::{
    gop::{CHAR_HEIGHT, CHAR_WIDTH, Pos, Screen},
    mouse::MOUSE_POINTER,
    psf1::PSF1Font,
};

pub struct TTY {
    // we use a vecdeque so that newline operations are cheap
    buffer: VecDeque<Line>,
    fg_color: u32,
    bg_color: u32,
    dims_x: usize,
    dims_y: usize,
    pos_x: usize,
    pos_y: usize,
    dirty_box: Option<BoundingBox>,
}

pub struct Writer<'a> {
    pub screen: Screen<'a>,
    pub tty: TTY,
    pub mouse_pos: Pos,
    pub mouse_colour: u32,
}

impl<'a> Writer<'a> {
    pub fn new(gop: GopInfo, font: PSF1Font<'a>) -> Writer<'a> {
        let unicode_buffer = &font.unicode_buffer;

        let mut unicode_table: BTreeMap<char, usize> = BTreeMap::new();

        let mut index = 0;
        for byte_index in (0..unicode_buffer.len()).step_by(2) {
            let unicode_byte =
                (unicode_buffer[byte_index] as u16) | (unicode_buffer[byte_index + 1] as u16) << 8;

            if unicode_byte == 0xFFFF {
                index += 1;
            } else {
                unicode_table.insert(
                    char::from_u32(unicode_byte.into())
                        .expect("unicode table should only have valid chars"),
                    index,
                );
            }
        }
        Self {
            tty: TTY::new(gop.horizonal / CHAR_WIDTH, gop.vertical / CHAR_HEIGHT),
            mouse_pos: Pos { x: 0, y: 0 },
            screen: Screen {
                gop,
                font,
                unicode_table,
            },
            mouse_colour: 0xFF_FF_FF,
        }
    }
}

impl core::fmt::Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for c in s.chars() {
            self.tty.write_char(c);
        }
        Ok(())
    }
}

impl Writer<'_> {
    pub fn clear(&mut self) {
        self.tty.clear();
    }

    pub fn reset_screen(&mut self, color: u32) {
        self.tty.bg_color = color;
        self.clear();
        self.tty.pos_x = 0;
        self.tty.pos_y = 0;
    }

    pub fn update_cursor(&mut self, pos: Pos, colour: u32) {
        // clear the old cursor by resetting a box around the cursor.
        let p = self.mouse_pos;
        let min_x = (p.x / CHAR_WIDTH).saturating_sub(1);
        let min_y = (p.y / CHAR_HEIGHT).saturating_sub(1);
        for (y, l) in self.tty.buffer.iter().enumerate().skip(min_y).take(3) {
            for (x, c) in l.cells.iter().enumerate().skip(min_x).take(3) {
                self.screen.update_cell(c, x, y)
            }
        }

        self.mouse_pos = pos;
        self.mouse_colour = colour;

        self.screen.draw_cursor(pos, colour, MOUSE_POINTER);
    }

    pub fn redraw_if_needed(&mut self) {
        // redraw section of screen that has been modified
        if let Some(b) = self.tty.dirty_box.take() {
            let cursor_cell = (self.mouse_pos.y / CHAR_HEIGHT) + 1;
            let y_cells = self.tty.buffer.iter().enumerate();
            for (y, line) in y_cells.take(b.max_y).skip(b.min_y) {
                for (x, c) in line.cells.iter().enumerate().take(b.max_x).skip(b.min_x) {
                    self.screen.update_cell(c, x, y)
                }

                // prevent flicker by drawing cursor right after overwriting it
                if y == cursor_cell {
                    self.screen
                        .draw_cursor(self.mouse_pos, self.mouse_colour, MOUSE_POINTER);
                }
            }
            // make sure cursor was drawn
            self.screen
                .draw_cursor(self.mouse_pos, self.mouse_colour, MOUSE_POINTER);
        }
    }
}

impl TTY {
    pub fn new(dims_x: usize, dims_y: usize) -> Self {
        let mut buffer = VecDeque::with_capacity(dims_y);

        for _ in 0..dims_y {
            let mut newline = Vec::with_capacity(dims_x);

            for _ in 0..dims_x {
                newline.push(Cell {
                    chr: ' ',
                    fg: 0xFF_FF_FF,
                    bg: 0,
                })
            }

            buffer.push_back(Line {
                cells: newline.into_boxed_slice(),
            });
        }

        Self {
            buffer,
            fg_color: 0xFF_FF_FF,
            bg_color: 0,
            dims_x,
            dims_y,
            pos_x: 0,
            pos_y: 0,
            dirty_box: Some(BoundingBox::from_max(dims_x, dims_y)),
        }
    }

    pub fn set_fg_colour(&mut self, colour: u32) -> u32 {
        core::mem::replace(&mut self.fg_color, colour)
    }

    pub fn write_char(&mut self, chr: char) {
        match chr {
            '\n' => self.newline(),
            // Backspace control character
            '\x08' => {
                match (self.pos_x, self.pos_y) {
                    (0, 0) => {}
                    (0, _) => {
                        self.pos_y -= 1;
                        self.pos_x = self.dims_y - 1;
                    }
                    _ => {
                        self.pos_x -= 1;
                    }
                }
                let cell = &mut self.buffer[self.pos_y].cells[self.pos_x];
                cell.chr = ' ';
                self.set_cell_dirty(self.pos_x, self.pos_y);
            }
            chr => {
                let cell = &mut self.buffer[self.pos_y].cells[self.pos_x];
                // update cell properties
                cell.chr = chr;
                cell.fg = self.fg_color;
                cell.bg = self.bg_color;
                self.set_cell_dirty(self.pos_x, self.pos_y);
                self.advance_char();
            }
        }
    }

    pub fn clear(&mut self) {
        for line in self.buffer.iter_mut() {
            for chr in line.cells.iter_mut() {
                chr.chr = ' ';
                chr.bg = self.bg_color;
            }
        }
        self.set_complete_dirty();
    }

    fn advance_char(&mut self) {
        if self.pos_x + 1 == self.dims_x {
            self.newline();
        } else {
            self.pos_x += 1;
        }
    }

    fn newline(&mut self) {
        self.pos_x = 0;
        if self.pos_y + 1 == self.dims_y {
            self.buffer.rotate_left(1);
            // clear the last row
            for c in self.buffer[self.dims_y - 1].cells.iter_mut() {
                c.chr = ' ';
            }
            self.set_complete_dirty();
        } else {
            self.pos_y += 1;
        }
    }

    fn set_cell_dirty(&mut self, x: usize, y: usize) {
        match &mut self.dirty_box {
            None => {
                self.dirty_box = Some(BoundingBox {
                    min_x: x,
                    max_x: x + 1,
                    min_y: y,
                    max_y: y + 1,
                })
            }
            Some(b) => {
                b.min_x = core::cmp::min(b.min_x, x);
                b.max_x = core::cmp::max(b.max_x, x + 1);
                b.min_y = core::cmp::min(b.min_y, y);
                b.max_y = core::cmp::max(b.max_y, y + 1);
            }
        }
    }

    fn set_complete_dirty(&mut self) {
        self.dirty_box = Some(BoundingBox::from_max(self.dims_x, self.dims_y));
    }
}

pub struct Line {
    cells: Box<[Cell]>,
}

pub struct Cell {
    pub chr: char,
    pub fg: u32,
    pub bg: u32,
}

pub struct BoundingBox {
    min_x: usize,
    max_x: usize,
    min_y: usize,
    max_y: usize,
}

impl BoundingBox {
    pub const fn from_max(max_x: usize, max_y: usize) -> Self {
        BoundingBox {
            min_x: 0,
            max_x,
            min_y: 0,
            max_y,
        }
    }
}
