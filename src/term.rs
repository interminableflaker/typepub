use std::{
    borrow::Cow,
    cmp::Ordering,
    io::Write,
    ops::{Bound, RangeBounds},
    sync::Arc,
};

use crossterm::{
    cursor,
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute, queue,
    style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor},
    terminal,
};

use crate::{
    backend::{Backend, Len},
    epub::{Content, Epub},
    style::{Style, Styling},
};

const PARAGRAPH_TERMINATOR: &str = "↵";
// const PARAGRAPH_TERMINATOR: &str = "¬";
// const PARAGRAPH_TERMINATOR: &str = " ";

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Linebreak {
    Wrapped,
    Existing,
    Eof,
}

#[derive(Debug)]
struct VirtualLine {
    line: usize,
    start: Len,
    end: Len,
    separator_len: Len,
    linebreak: Linebreak,
}

#[derive(Debug)]
struct ScreenLine<'a> {
    line: &'a VirtualLine,
    row: u16,
}

impl ScreenLine<'_> {
    fn len(&self) -> Len {
        self.line.end - self.line.start
    }

    fn len_with_break(&self) -> Len {
        self.line.end - self.line.start + self.line.separator_len
    }
}

enum State {
    ChapterSelect,
    Chapter(ChapterDisplay),
}

struct Dimensions {
    screen_size: (u16, u16),
    anchor: (u16, u16),
    width: u16,
}

pub struct Display {
    dimensions: Arc<Dimensions>,
    book: Epub,
    chapter: usize,
    state: State,
}

impl Display {
    pub fn new(book: Epub, width: u16, view_width: u16, view_height: u16) -> Self {
        let width = width.min(view_width);

        Self {
            state: State::ChapterSelect,
            book,
            chapter: 0,
            dimensions: Arc::new(Dimensions {
                screen_size: (view_width, view_height),
                anchor: (view_width / 2 - width / 2, view_height / 2),
                width,
            }),
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
        execute!(w, ResetColor, cursor::Show, terminal::LeaveAlternateScreen)?;
        Ok(())
    }

    pub fn render(&mut self, w: &mut impl Write) -> anyhow::Result<()> {
        match &mut self.state {
            State::ChapterSelect { .. } => self.full_render(w)?,
            State::Chapter(display) => {
                if display.render_chapter(w)? {
                    self.full_render(w)?;
                }
            }
        }
        Ok(())
    }

    fn full_render(&mut self, w: &mut impl Write) -> anyhow::Result<()> {
        match &mut self.state {
            State::ChapterSelect => {
                queue!(w, cursor::Hide, terminal::Clear(terminal::ClearType::All))?;

                let chapter = self.book.chapters().nth(self.chapter).unwrap();
                let depth_offset = 2 * chapter.depth();
                let wrap_at = self.content_width() as usize - depth_offset;
                let wrapped = textwrap::wrap(chapter.name(), wrap_at);
                let line = self.middle_row() - (wrapped.len() as u16 - 1) / 2;
                queue!(
                    w,
                    cursor::MoveTo(self.content_starting_col() - 2, self.middle_row())
                )?;
                w.write_all(b">")?;
                for (i, wrap) in wrapped.iter().enumerate() {
                    queue!(
                        w,
                        cursor::MoveTo(
                            self.content_starting_col() + depth_offset as u16,
                            line + i as u16
                        )
                    )?;
                    w.write_all(wrap.as_bytes())?;
                }

                let mut above = line - 2;
                let mut below = line + wrapped.len() as u16 + 1;

                'outer: for chapter in self
                    .book
                    .chapters()
                    .rev()
                    .skip(self.book.chapter_count() - self.chapter)
                {
                    let depth_offset = 2 * chapter.depth();
                    let wrap_at = self.content_width() as usize - depth_offset;
                    let wrapped = textwrap::wrap(chapter.name(), wrap_at);

                    for (i, wrap) in wrapped.iter().rev().enumerate() {
                        queue!(
                            w,
                            cursor::MoveTo(
                                self.content_starting_col() + depth_offset as u16,
                                above - i as u16
                            )
                        )?;
                        w.write_all(wrap.as_bytes())?;
                        if above <= 1 + i as u16 {
                            break 'outer;
                        }
                    }

                    above -= u16::try_from(wrapped.len()).unwrap() + 1;
                }
                'outer: for chapter in self.book.chapters().skip(self.chapter + 1) {
                    let depth_offset = 2 * chapter.depth();
                    let wrap_at = self.content_width() as usize - depth_offset;
                    let wrapped = textwrap::wrap(chapter.name(), wrap_at);

                    for (i, wrap) in wrapped.iter().enumerate() {
                        if below + i as u16 >= self.screen_height() {
                            break 'outer;
                        }
                        queue!(
                            w,
                            cursor::MoveTo(
                                self.content_starting_col() + depth_offset as u16,
                                below + i as u16
                            )
                        )?;
                        w.write_all(wrap.as_bytes())?;
                    }

                    below += u16::try_from(wrapped.len()).unwrap() + 1;
                }
                w.flush()?;
                Ok(())
            }
            State::Chapter(display) => display.full_render_chapter(w),
        }
    }

    pub fn handle_input(&mut self, event: KeyEvent) -> anyhow::Result<bool> {
        if let KeyEvent {
            code: KeyCode::Esc, ..
        } = &event
        {
            match &mut self.state {
                State::ChapterSelect => return Ok(true),
                State::Chapter(..) => {
                    self.state = State::ChapterSelect;
                    return Ok(false);
                }
            }
        }
        match &mut self.state {
            State::ChapterSelect => match event {
                KeyEvent {
                    code: KeyCode::Up | KeyCode::Char('k'),
                    ..
                } => self.chapter = self.chapter.saturating_sub(1),
                KeyEvent {
                    code: KeyCode::Down | KeyCode::Char('j'),
                    ..
                } => {
                    self.chapter =
                        (self.chapter + 1).min(self.book.chapter_count().saturating_sub(1))
                }
                KeyEvent {
                    code: KeyCode::Enter,
                    ..
                } => {
                    self.state = State::Chapter(ChapterDisplay::enter(
                        Arc::clone(&self.dimensions),
                        &mut self.book,
                        self.chapter,
                    ));
                }
                _ => {}
            },
            State::Chapter(display) => display.handle_input(event)?,
        }
        Ok(false)
    }
}

struct ChapterDisplay {
    dimensions: Arc<Dimensions>,
    backend: Backend,
    lines: Vec<VirtualLine>,
    styling: Styling<Len>,
    previous_line: usize,
    needs_full_render: bool,
}

trait DisplayState {
    fn dimensions(&self) -> &Dimensions;

    fn content_width(&self) -> u16 {
        self.dimensions().width
    }

    fn screen_width(&self) -> u16 {
        self.dimensions().screen_size.0
    }

    fn screen_height(&self) -> u16 {
        self.dimensions().screen_size.1
    }

    fn content_starting_col(&self) -> u16 {
        self.dimensions().anchor.0
    }

    fn middle_row(&self) -> u16 {
        self.dimensions().anchor.1
    }
}

impl DisplayState for Display {
    fn dimensions(&self) -> &Dimensions {
        &self.dimensions
    }
}

impl DisplayState for ChapterDisplay {
    fn dimensions(&self) -> &Dimensions {
        &self.dimensions
    }
}

impl ChapterDisplay {
    pub fn enter(dimensions: Arc<Dimensions>, book: &mut Epub, chapter: usize) -> Self {
        // TODO more robust solution, this is all temporary
        let mut text = String::new();
        let mut char_count = 0;
        let mut styling = Styling::builder();
        book.traverse(chapter, |content| match content {
            Content::Text(style, mut s) => {
                if matches!(text.chars().last(), None | Some('\n')) {
                    s = s.trim_start();
                }
                const REPLACEMENTS: &[(char, &str)] = &[('—', "--"), ('…', "...")];
                let mut s = Cow::Borrowed(s);
                for &(c, rep) in REPLACEMENTS {
                    if s.contains(c) {
                        s = s.replace(c, rep).into();
                    }
                }

                let len_chars = s.chars().count();
                let start = Len::new(text.len(), char_count);
                let end = Len::new(start.bytes + s.len(), start.chars + len_chars);
                styling.add(style, start..end);
                char_count += len_chars;
                text.push_str(&s);
            }
            Content::Linebreak => {
                while matches!(text.chars().last(), Some(c) if c.is_whitespace()) {
                    char_count -= 1;
                    text.pop();
                }
                char_count += 1;
                text.push('\n');
            }
            Content::Image => {
                let img_text = "img";
                char_count += img_text.chars().count();
                text.push_str(img_text);
            }
            Content::Title => todo!(),
        })
        .unwrap();
        let lines = Self::wrap_text(&text, dimensions.width);

        Self {
            dimensions,
            backend: Backend::new(text),
            lines,
            styling: styling.build(),
            previous_line: 0,
            needs_full_render: true,
        }
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
                (Len::new(len, separator.chars().count()), kind)
            };
            lines.push(VirtualLine {
                line: this_line,
                start: Len::new(byte_sum, char_sum),
                end: Len::new(end, char_sum + line_chars),
                separator_len,
                linebreak,
            });
            byte_sum += line.len() + separator_len.bytes;
            char_sum += line_chars + separator_len.chars;
            prev = Some(next);
        }
        lines.push(VirtualLine {
            line: line_number,
            start: Len::new(byte_sum, char_sum),
            end: Len::new(text.len(), char_sum + text[byte_sum..].chars().count()),
            separator_len: Len::new(0, 0),
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
        .min(self.screen_height() - 1);
        let end_bound = match range.end_bound() {
            Bound::Included(&l) => l + 1,
            Bound::Excluded(&l) => l,
            Bound::Unbounded => self.screen_height(),
        }
        .min(self.screen_height());

        let line = self.char_index_to_virtual_line(self.backend.cursor().chars);
        let cursor_vln = self.lines[line].line;
        let top_of_screen_vln = cursor_vln as isize - self.middle_row() as isize;
        let start_vln = (top_of_screen_vln + start_bound as isize).max(0) as usize;
        let end_vln = (top_of_screen_vln + end_bound as isize).max(0) as usize;
        let offset = (start_vln as isize - top_of_screen_vln).max(0) as usize;

        let heuristic = match start_bound <= self.middle_row() {
            true => line.saturating_sub((self.middle_row() - start_bound) as usize),
            false => line + (start_bound - self.middle_row()) as usize / 2,
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
        crossterm::queue!(
            w,
            SetAttribute(Attribute::Reverse),
            SetForegroundColor(Color::Red),
        )?;
        cb(w)?;
        crossterm::queue!(
            w,
            SetForegroundColor(Color::Reset),
            SetAttribute(Attribute::NoReverse),
        )?;
        Ok(())
    }

    fn render_line(&self, w: &mut impl Write, line: &ScreenLine) -> anyhow::Result<()> {
        self.render_range_in_line(w, line, Len::new(0, 0), line.len_with_break())
    }

    fn render_range_in_line(
        &self,
        w: &mut impl Write,
        line: &ScreenLine,
        start: Len,
        end: Len,
    ) -> anyhow::Result<()> {
        queue!(
            w,
            cursor::MoveTo(self.content_starting_col() + start.chars as u16, line.row)
        )?;
        let slice_end = end.min(line.len());
        let mut text = self.virtual_line_str(line.line)[start.bytes..slice_end.bytes].as_bytes();
        let mut cur_style = Style::empty();
        for (style, len) in self
            .styling
            .iter(line.line.start + start, line.line.start + slice_end)
        {
            for attr in (cur_style & !style)
                .iter()
                .filter_map(|s| match s {
                    Style::BOLD => Some(Attribute::NormalIntensity),
                    Style::ITALIC => Some(Attribute::NoItalic),
                    _ => None,
                })
                .chain((style & !cur_style).iter().filter_map(|s| match s {
                    Style::BOLD => Some(Attribute::Bold),
                    Style::ITALIC => Some(Attribute::Italic),
                    _ => None,
                }))
            {
                crossterm::queue!(w, SetAttribute(attr))?;
            }
            w.write_all(&text[..len.bytes])?;
            text = &text[len.bytes..];
            cur_style = style;
        }
        if line.line.linebreak == Linebreak::Existing && end > slice_end {
            write!(w, "{PARAGRAPH_TERMINATOR}")?;
        }
        // TODO: this also disables error coloring
        crossterm::queue!(w, SetAttribute(Attribute::Reset))?;
        Ok(())
    }

    fn line_difference(&self, current_line: usize) -> isize {
        self.lines[current_line].line as isize - self.lines[self.previous_line].line as isize
    }

    // true -> needs full render
    pub fn render_chapter(&mut self, w: &mut impl Write) -> anyhow::Result<bool> {
        if self.needs_full_render {
            return Ok(true);
        }
        let (x, y) = self.to_virtual(self.backend.cursor().chars);
        let line_diff = self.line_difference(y);
        let Ok(lines_scrolled) = u16::try_from(line_diff.abs()) else {
            return Ok(true);
        };

        queue!(w, cursor::Hide)?;

        if lines_scrolled > 0 {
            let range = if y > self.previous_line {
                queue!(w, terminal::ScrollUp(lines_scrolled))?;
                let bottom = self.screen_height();
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
        // error highlighting
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
                Ordering::Greater => self.middle_row() - lines_scrolled..=self.middle_row(),
                _ => self.middle_row()..=self.middle_row() + lines_scrolled,
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
                    let len = Len::new(
                        self.backend.text()[err.bytes..]
                            .chars()
                            .next()
                            .unwrap()
                            .len_utf8(),
                        1,
                    );
                    match cursor_pos.chars < last_cursor_pos.chars {
                        true => self.render_range_in_line(w, &line, x, x + len)?,
                        false => {
                            self.with_error(w, |w| self.render_range_in_line(w, &line, x, x + len))?
                        }
                    }
                    cur += 1;
                }
            }
        }

        queue!(
            w,
            cursor::MoveTo(self.content_starting_col() + x, self.middle_row()),
            cursor::Show,
        )?;

        w.flush()?;
        self.previous_line = y;
        self.backend.clear_per_update_data();
        Ok(false)
    }

    fn full_render_chapter(&mut self, w: &mut impl Write) -> anyhow::Result<()> {
        let (x, _y) = self.to_virtual(self.backend.cursor().chars);

        queue!(w, cursor::Hide, terminal::Clear(terminal::ClearType::All))?;
        for line in self.screen_lines(..) {
            self.render_line(w, &line)?;
        }
        queue!(
            w,
            cursor::MoveTo(self.content_starting_col() + x, self.middle_row()),
            cursor::Show,
        )?;
        w.flush()?;
        self.needs_full_render = false;
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
