use std::borrow::Cow;

use crate::{
    epub::{Content, Epub},
    style::{Style, Styling},
};

#[rustfmt::skip]
const REPLACEMENTS: &[(char, &str)] = &[
    ('—', "--"),
    ('…', "..."),
];

#[rustfmt::skip]
const ALTERNATIVES: &[(char, &[char])] = &[
    ('\'', &['‘', '’']),
    ('\"', &['“', '”']),
    (' ', &[' '])
];

pub struct Backend {
    text: String,
    typed: String,
    cursor: Len,
    cursor_prev: Len,
    errors: Vec<Len>,
    deleted_errors: Vec<Len>,
    styling: Styling<Len>,
}

impl Backend {
    pub fn new(book: &mut Epub, chapter: usize) -> Self {
        let mut text = String::new();
        let mut char_count = 0;
        let mut styling = Styling::builder();

        book.traverse(chapter, |content| match content {
            Content::Text(style, mut s) => {
                if matches!(text.chars().last(), None | Some('\n')) {
                    s = s.trim_start();
                }
                let s = replace_unicode_multichars(&mut s);
                let len_chars = s.chars().count();
                let start = Len::new(text.len(), char_count);
                let end = Len::new(start.bytes + s.len(), start.chars + len_chars);
                styling.add(style, start..end);
                char_count += len_chars;
                text.push_str(&s);
            }
            Content::Linebreak => {
                char_count -= trim_end_in_place(&mut text);
                if !matches!(text.chars().last(), None) {
                    char_count += 1;
                    text.push('\n');
                }
            }
            Content::Image => {
                // let img_text = "img";
                // char_count += img_text.chars().count();
                // text.push_str(img_text);
            }
            Content::Title => todo!(),
        })
        .unwrap();
        trim_end_in_place(&mut text);

        Self {
            text,
            typed: String::new(),
            cursor: Len::new(0, 0),
            cursor_prev: Len::new(0, 0),
            errors: Vec::new(),
            deleted_errors: Vec::new(),
            styling: styling.build(),
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> Len {
        self.cursor
    }

    pub fn last_cursor_position(&self) -> Len {
        self.cursor_prev
    }

    pub fn errors(&self) -> &[Len] {
        &self.errors
    }

    pub fn backspaced_errors(&self) -> &[Len] {
        &self.deleted_errors
    }

    pub fn clear_per_update_data(&mut self) {
        self.deleted_errors.truncate(0);
    }

    pub fn push(&mut self, c: char) {
        let Some(goal) = self.text[self.cursor.bytes..].chars().next() else {
            return;
        };
        self.typed.push(c);
        if !chars_are_equal_including_unicode_alternatives(goal, c) {
            self.errors.push(self.cursor);
        }
        self.cursor_prev = self.cursor;
        self.cursor.bytes += goal.len_utf8();
        self.cursor.chars += 1;
    }

    pub fn pop(&mut self) {
        let Some(typed) = self.typed.chars().last() else {
            return;
        };
        let text = self.text[..self.cursor.bytes].chars().last().unwrap();
        self.delete_backwards_impl(Len::new(text.len_utf8(), 1), Len::new(typed.len_utf8(), 1));
    }

    pub fn delete_word_backwards(&mut self) {
        let mut found_nonwhitespace = false;
        let [typed, text] = self
            .typed
            .chars()
            .rev()
            .take_while(move |c| {
                let is_ws = c.is_whitespace();
                found_nonwhitespace |= !is_ws;
                !(found_nonwhitespace && is_ws)
            })
            .zip(self.text[..self.cursor.bytes].chars().rev())
            .map(|(a, b)| [Len::new(a.len_utf8(), 1), Len::new(b.len_utf8(), 1)])
            .fold([Len::default(); 2], |acc, x| [0, 1].map(|i| acc[i] + x[i]));
        self.delete_backwards_impl(text, typed);
    }

    fn delete_backwards_impl(&mut self, len: Len, typed: Len) {
        self.typed.truncate(self.typed.len() - typed.bytes);
        self.cursor_prev = self.cursor;
        self.cursor -= len;

        // TODO: binary search this
        if let Some(first_deleted_error) = self
            .errors
            .iter()
            .position(|&i| i.chars >= self.cursor.chars)
        {
            self.deleted_errors
                .extend(self.errors.drain(first_deleted_error..));
        }
    }

    pub fn style_iter(&self, start: Len, end: Len) -> impl Iterator<Item = (Style, Len)> + '_ {
        self.styling.iter(start, end)
    }
}

#[derive(Debug, Copy, Clone, Default, PartialEq, Eq)]
pub struct Len {
    pub bytes: usize,
    pub chars: usize,
}

impl std::cmp::PartialOrd for Len {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.bytes.cmp(&other.bytes))
    }
}

impl std::cmp::Ord for Len {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.bytes.cmp(&other.bytes)
    }
}

impl Len {
    pub fn new(bytes: usize, chars: usize) -> Self {
        Self { bytes, chars }
    }
}

impl std::ops::Add<Self> for Len {
    type Output = Self;

    fn add(self, Self { bytes, chars }: Self) -> Self::Output {
        Self {
            bytes: self.bytes + bytes,
            chars: self.chars + chars,
        }
    }
}

impl std::ops::AddAssign<Self> for Len {
    fn add_assign(&mut self, rhs: Self) {
        self.bytes += rhs.bytes;
        self.chars += rhs.chars;
    }
}

impl std::ops::Sub<Self> for Len {
    type Output = Self;

    fn sub(self, Self { bytes, chars }: Self) -> Self::Output {
        Self {
            bytes: self.bytes - bytes,
            chars: self.chars - chars,
        }
    }
}

impl std::ops::SubAssign<Self> for Len {
    fn sub_assign(&mut self, rhs: Self) {
        self.bytes -= rhs.bytes;
        self.chars -= rhs.chars;
    }
}

fn chars_are_equal_including_unicode_alternatives(expected: char, got: char) -> bool {
    if expected == got {
        true
    } else if let Some(alts) = ALTERNATIVES.iter().find(|x| x.0 == got) {
        alts.1.contains(&expected)
    } else {
        false
    }
}

fn replace_unicode_multichars(s: &str) -> Cow<str> {
    let mut s = Cow::Borrowed(s);
    for &(c, rep) in REPLACEMENTS {
        if s.contains(c) {
            s = s.replace(c, rep).into();
        }
    }
    s
}

fn trim_end_in_place(s: &mut String) -> usize {
    let mut count = 0;
    while matches!(s.chars().last(), Some(c) if c.is_whitespace()) {
        count += 1;
        s.pop();
    }
    count
}
