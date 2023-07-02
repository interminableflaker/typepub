use std::{
    num::NonZeroUsize,
    path::PathBuf,
};

use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use typepub::{
    epub::{Directory, SearchBackend, Epub},
    term::Display,
};

// TODO: features
// - render styles
// - nicer virtual styling
// - chapter select
// - book select maybe?
// - progress saving
// - scorescreen; wpm/acc display at end
// - score annotations per paragraph
// - window resize
// - sixel images

// TODO: annoyances/bugs
// - british english uses single quotation marks for speech, prefer double--
//   convert single quotation marks to double quotations? messes up quotes
//   within quotes, so will have to parse nested and convert accordingly

fn main() -> anyhow::Result<()> {
    xflags::xflags! {
        cmd typepub {
            cmd path {
                /// Path to book.
                required path: PathBuf
                /// Chapter to open.
                required chapter: usize
            }
            cmd search {
                /// Book name to search for. Case insensitive.
                required search: String
                /// Chapter to open.
                required chapter: usize
                /// Optional directory to search for books.
                /// Defaults
                ///     Unix:    `$HOME/books`
                ///     Windows: `%HOMEPATH%\Documents\books`
                optional -l,--library library: PathBuf
            }
            /// Width of text view, in characters.
            /// Defaults to 80.
            optional -w,--width width: NonZeroUsize
        }
    };

    let args = Typepub::from_env()?;
    let (book, chapter) = match args.subcommand {
        TypepubCmd::Path(Path{path, chapter}) => {
            (Epub::from_path(&path)?, chapter)
        }
        TypepubCmd::Search(Search{library, search, chapter}) => {
            let book = library
                .map_or_else(Directory::from_home, Directory::from_path)?
                .search(&search)?
                .context("book not found")?;
            (book, chapter)
        }
    };

    let width = args.width.and_then(|x| x.get().try_into().ok()).unwrap_or(80u16);

    println!("{}'s {}", book.author().unwrap(), book.name());

    let (term_w, term_h) = crossterm::terminal::size()?;

    let mut w = std::io::stdout();
    let mut display = Display::new(book, chapter, width, term_w, term_h);

    display.enter(&mut w)?;

    loop {
        match next_key_event()? {
            KeyEvent {
                code: KeyCode::Esc, ..
            } => break,
            e => display.handle_input(e)?,
        }

        display.render(&mut w)?;
    }

    display.exit(&mut w)?;

    Ok(())
}

fn next_key_event() -> anyhow::Result<KeyEvent> {
    loop {
        if let Ok(Event::Key(event)) = event::read() {
            return Ok(event);
        }
    }
}
