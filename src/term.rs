use std::{
    borrow::Cow,
    cmp::Ordering,
    io::Write,
    ops::{Bound, RangeBounds},
};

use crossterm::{
    cursor,
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute, queue, style, terminal,
};

use crate::{
    backend::{Backend, TextPosition},
    epub::{Content, Epub},
};

// const PARAGRAPH_TERMINATOR: &str = "↵";
// const PARAGRAPH_TERMINATOR: &str = "¬";
const PARAGRAPH_TERMINATOR: &str = " ";

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Linebreak {
    Wrapped,
    Existing,
    Eof,
}

#[derive(Debug)]
struct VirtualLine {
    line: usize,
    start: TextPosition,
    end: TextPosition,
    separator_len: TextPosition,
    linebreak: Linebreak,
}

#[derive(Debug)]
struct ScreenLine<'a> {
    line: &'a VirtualLine,
    row: u16,
}

impl ScreenLine<'_> {
    fn len_bytes(&self) -> usize {
        self.line.end.bytes - self.line.start.bytes
    }
}

enum State {
    // BookSelect,
    ChapterSelect(Epub),
    Chapter(Epub, usize),
}

pub struct Display {
    state: State,
    backend: Backend,
    screen_size: (u16, u16),
    anchor: (u16, u16),
    lines: Vec<VirtualLine>,
    previous_line: usize,
}

impl Display {
    pub fn new(mut epub: Epub, chapter: usize, width: u16, view_width: u16, view_height: u16) -> Self {
        let width = width.min(view_width);

        // TODO more robust solution, this is all temporary
        let mut text = String::new();
        epub.traverse(chapter, |content| match content {
            Content::Text(_, s) => {
                const REPLACEMENTS: &[(char, &str)] = &[('—', "--"), ('…', " ... ")];
                let mut s = Cow::Borrowed(s);
                for &(c, rep) in REPLACEMENTS {
                    if s.contains(c) {
                        s = s.replace(c, rep).into();
                    }
                }
                // println!("{}", s);
                text.push_str(&s);
            }
            Content::Linebreak => {
                while matches!(text.chars().last(), Some(c) if c.is_whitespace()) {
                    text.pop();
                }
                text.push('\n');
            }
            Content::Image => text.push_str("img"),
            Content::Title => todo!(),
        })
        .unwrap();
        let text = text.trim().to_owned();
        // panic!("{:#?}", text);
        let lines = Self::wrap_text(&text, width);

        Self {
            state: State::ChapterSelect(epub),
            backend: Backend::new(text),
            screen_size: (view_width, view_height),
            anchor: (view_width / 2 - width / 2, view_height / 2),
            previous_line: 0,
            lines,
        }
    }

    pub fn enter(&mut self, w: &mut impl Write) -> anyhow::Result<()> {
        execute!(w, terminal::EnterAlternateScreen, cursor::Hide)?;
        terminal::enable_raw_mode()?;
        let hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info: &std::panic::PanicInfo| {
            let _ = Self::cleanup(&mut std::io::stdout());
            hook(info);
        }));
        self.full_render(w)?;
        Ok(())
    }

    pub fn exit(&self, w: &mut impl Write) -> anyhow::Result<()> {
        Self::cleanup(w)?;
        let _ = std::panic::take_hook();
        Ok(())
    }

    fn cleanup(w: &mut impl Write) -> anyhow::Result<()> {
        terminal::disable_raw_mode()?;
        execute!(
            w,
            style::ResetColor,
            cursor::Show,
            terminal::LeaveAlternateScreen
        )?;
        Ok(())
    }

    fn wrap_text(text: &str, width: u16) -> Vec<VirtualLine> {
        let mut lines = vec![];
        let mut byte_sum = 0;
        let mut char_sum = 0;
        let mut line_number = 0;

        let wrapped = textwrap::wrap(text, width as usize);
        let mut it = wrapped.into_iter();
        let mut prev = it.next();
        while let (Some(line), Some(next)) = (prev, it.next()) {
            let this_line = line_number;
            let line_chars = line.chars().count();
            let end = byte_sum + line.len();
            let (separator_len, linebreak) = {
                let next_start = next.as_ptr() as usize - text.as_ptr() as usize;
                let len = next_start - end;
                let separator = &text[end..next_start];
                let kind = match separator.contains('\n') {
                    true => {
                        line_number += 2;
                        Linebreak::Existing
                    }
                    false => {
                        line_number += 1;
                        Linebreak::Wrapped
                    }
                };
                (TextPosition::new(len, separator.chars().count()), kind)
            };
            lines.push(VirtualLine {
                line: this_line,
                start: TextPosition::new(byte_sum, char_sum),
                end: TextPosition::new(end, char_sum + line_chars),
                separator_len,
                linebreak,
            });
            byte_sum += line.len() + separator_len.bytes;
            char_sum += line_chars + separator_len.chars;
            prev = Some(next);
        }
        lines.push(VirtualLine {
            line: line_number,
            start: TextPosition::new(byte_sum, char_sum),
            end: TextPosition::new(text.len(), char_sum + text[byte_sum..].chars().count()),
            separator_len: TextPosition::new(0, 0),
            linebreak: Linebreak::Eof,
        });
        lines
    }

    fn char_index_to_virtual_line(&self, idx: usize) -> usize {
        self.lines
            .binary_search_by(|element| match element.start.chars.cmp(&idx) {
                Ordering::Equal => Ordering::Less,
                ord => ord,
            })
            .unwrap_err()
            .saturating_sub(1)
    }

    fn to_virtual(&self, cursor: usize) -> (u16, usize) {
        let y = self.char_index_to_virtual_line(cursor);
        let x = cursor - self.lines[y].start.chars;
        (x.try_into().unwrap(), y)
    }

    fn virtual_line_str(&self, vl: &VirtualLine) -> &str {
        &self.backend.text()[vl.start.bytes..vl.end.bytes]
    }

    fn screen_lines(&self, range: impl RangeBounds<u16>) -> impl Iterator<Item = ScreenLine<'_>> {
        let start_bound = match range.start_bound() {
            Bound::Included(&l) => l,
            Bound::Excluded(&l) => l + 1,
            Bound::Unbounded => 0,
        }
        .min(self.screen_size.1 - 1);
        let end_bound = match range.end_bound() {
            Bound::Included(&l) => l + 1,
            Bound::Excluded(&l) => l,
            Bound::Unbounded => self.screen_size.1,
        }
        .min(self.screen_size.1);

        let line = self.char_index_to_virtual_line(self.backend.cursor().chars);
        let cursor_vln = self.lines[line].line;
        let top_of_screen_vln = cursor_vln as isize - self.anchor.1 as isize;
        let start_vln = (top_of_screen_vln + start_bound as isize).max(0) as usize;
        let end_vln = (top_of_screen_vln + end_bound as isize).max(0) as usize;
        let offset = (start_vln as isize - top_of_screen_vln).max(0) as usize;

        let heuristic = match start_bound <= self.anchor.1 {
            true => line.saturating_sub((self.anchor.1 - start_bound) as usize),
            false => line + (start_bound - self.anchor.1) as usize / 2,
        };

        self.lines
            .get(heuristic..)
            .and_then(|ls| ls.iter().position(|l| l.line >= start_vln))
            .and_then(|idx| self.lines.get(idx + heuristic..))
            .into_iter()
            .flatten()
            .take_while(move |vl| vl.line < end_vln)
            .map(move |vl| ScreenLine {
                line: vl,
                row: ((vl.line - start_vln) + offset) as u16,
            })
    }

    fn with_error<W>(
        &self,
        w: &mut W,
        cb: impl Fn(&mut W) -> anyhow::Result<()>,
    ) -> anyhow::Result<()>
    where
        W: Write,
    {
        w.write_all(b"\x1b[7;31m")?;
        cb(w)?;
        w.write_all(b"\x1b[0m")?;
        Ok(())
    }

    fn render_line(&self, w: &mut impl Write, line: &ScreenLine) -> anyhow::Result<()> {
        queue!(w, cursor::MoveTo(self.anchor.0, line.row))?;
        w.write_all(self.virtual_line_str(line.line).as_bytes())?;
        if line.line.linebreak == Linebreak::Existing {
            write!(w, "{PARAGRAPH_TERMINATOR}")?;
        }
        Ok(())
    }

    fn render_range_in_line(
        &self,
        w: &mut impl Write,
        line: &ScreenLine,
        start: TextPosition,
        end_bytes: usize,
    ) -> anyhow::Result<()> {
        queue!(
            w,
            cursor::MoveTo(self.anchor.0 + start.chars as u16, line.row)
        )?;
        let slice_end = end_bytes.min(line.len_bytes());
        let slice = &self.virtual_line_str(line.line)[start.bytes..slice_end];
        if !slice.is_empty() {
            w.write_all(slice.as_bytes())?;
        }
        if line.line.linebreak == Linebreak::Existing && end_bytes > slice_end {
            write!(w, "{PARAGRAPH_TERMINATOR}")?;
        }
        Ok(())
    }

    fn line_difference(&self, current_line: usize) -> isize {
        self.lines[current_line].line as isize - self.lines[self.previous_line].line as isize
    }

    pub fn render(&mut self, w: &mut impl Write) -> anyhow::Result<()> {
        let (x, y) = self.to_virtual(self.backend.cursor().chars);
        let line_diff = self.line_difference(y);
        let Ok(lines_scrolled) = u16::try_from(line_diff.abs()) else {
            return self.full_render(w);
        };

        queue!(w, cursor::Hide)?;

        if lines_scrolled > 0 {
            let range = if y > self.previous_line {
                queue!(w, terminal::ScrollUp(lines_scrolled))?;
                let bottom = self.screen_size.1;
                bottom - lines_scrolled..bottom
            } else {
                queue!(w, terminal::ScrollDown(lines_scrolled))?;
                0..lines_scrolled
            };
            for line in self.screen_lines(range) {
                self.render_line(w, &line)?;
            }
        }

        // if self.cursor_prev.bytes >= self.line_starts[self.previous_line].end.bytes {
        //     let x = self.cursor_prev.chars - self.line_starts[self.previous_line].start.chars;
        //     queue!(
        //         w,
        //         cursor::MoveTo(
        //             self.anchor.0 + x as u16,
        //             (self.anchor.1 as isize - line_diff) as u16
        //         ),
        //     )?;
        //     write!(w, " ")?;
        // }
        // // render new-line indicator if necessary
        // if self.cursor.bytes >= self.line_starts[y].end.bytes {
        //     queue!(w, cursor::MoveTo(self.anchor.0 + x as u16, self.anchor.1),)?;
        //     write!(w, "↵")?;
        //     queue!(w, cursor::MoveLeft(1))?;
        // }

        // TODO: too much complexity
        let cursor_pos = self.backend.cursor();
        let last_cursor_pos = self.backend.last_cursor_position();
        if let Some(errors) = if cursor_pos.chars > last_cursor_pos.chars {
            let errors = self.backend.errors();
            errors
                .len()
                .checked_sub(1)
                .and_then(|i| errors.get(i..))
                .filter(|e| e[0].bytes >= last_cursor_pos.bytes)
        } else {
            Some(self.backend.backspaced_errors())
        } {
            let range = match y.cmp(&self.previous_line) {
                Ordering::Greater => self.anchor.1 - lines_scrolled..=self.anchor.1,
                _ => self.anchor.1..=self.anchor.1 + lines_scrolled,
            };
            let mut cur = 0;
            'outer: for line in self.screen_lines(range) {
                let end = line.line.end + line.line.separator_len;
                loop {
                    let err = match errors.get(cur) {
                        Some(&e) if e.chars < end.chars => e,
                        Some(_) => break,
                        None => break 'outer,
                    };
                    let x = err - line.line.start;
                    let len_bytes = self.backend.text()[err.bytes..]
                        .chars()
                        .next()
                        .unwrap()
                        .len_utf8();
                    match cursor_pos.chars < last_cursor_pos.chars {
                        true => self.render_range_in_line(w, &line, x, x.bytes + len_bytes)?,
                        false => self.with_error(w, |w| {
                            self.render_range_in_line(w, &line, x, x.bytes + len_bytes)
                        })?,
                    }
                    cur += 1;
                }
            }
        }

        queue!(
            w,
            cursor::MoveTo(self.anchor.0 + x, self.anchor.1),
            cursor::Show,
        )?;

        w.flush()?;
        self.previous_line = y;
        self.backend.clear_per_update_data();
        Ok(())
    }

    fn full_render(&mut self, w: &mut impl Write) -> anyhow::Result<()> {
        let (x, _y) = self.to_virtual(self.backend.cursor().chars);

        queue!(w, cursor::Hide, terminal::Clear(terminal::ClearType::All))?;
        for line in self.screen_lines(..) {
            self.render_line(w, &line)?;
        }
        queue!(
            w,
            cursor::MoveTo(self.anchor.0 + x, self.anchor.1),
            cursor::Show,
        )?;
        w.flush()?;
        Ok(())
    }

    pub fn handle_input(&mut self, event: KeyEvent) -> anyhow::Result<()> {
        match event {
            KeyEvent {
                code: KeyCode::Backspace | KeyCode::Char('w'),
                modifiers: KeyModifiers::CONTROL,
                ..
            } => self.backend.delete_word_backwards(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => self.backend.pop(),
            KeyEvent {
                code: KeyCode::Enter,
                ..
            } => self.backend.push('\n'),
            KeyEvent {
                code: KeyCode::Char(c),
                ..
            } => self.backend.push(c),
            _ => {}
        }
        Ok(())
    }
}
