use std::{num::NonZeroUsize, path::PathBuf};

use anyhow::Context;
use crossterm::event::{self, Event, KeyEvent};
use typepub::{
    epub::{Directory, Epub, SearchBackend},
    term::Display,
};

// TODO: features
// - nicer virtual styling
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
            }
            cmd search {
                /// Book name to search for. Case insensitive.
                required search: String
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
    let book = match args.subcommand {
        TypepubCmd::Path(Path { path }) => Epub::from_path(&path)?,
        TypepubCmd::Search(Search { library, search }) => library
            .map_or_else(Directory::from_home, Directory::from_path)?
            .search(&search)?
            .context("book not found")?,
    };

    let width = args
        .width
        .and_then(|x| x.get().try_into().ok())
        .unwrap_or(80u16);

    println!("{}'s {}", book.author().unwrap(), book.name());

    let (term_w, term_h) = crossterm::terminal::size()?;

    let mut w = std::io::stdout();
    let mut display = Display::new(book, width, term_w, term_h);

    display.enter(&mut w)?;

    loop {
        let ev = next_key_event()?;
        if display.handle_input(ev)? {
            break;
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
