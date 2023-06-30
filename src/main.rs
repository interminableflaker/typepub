use anyhow::Context;
use crossterm::event::{self, Event, KeyCode, KeyEvent};
use typepub::{
    epub::{Directory, SearchBackend},
    term::Display,
};

// TODO
// - render styles
// - convert empty newlines into virtual only
// - book loading
// - book select?
// - chapter select
// - progress saving
// - scorescreen; wpm/acc display at end
// - score annotations per paragraph
// - window resize

fn main() -> anyhow::Result<()> {
    let book = Directory::from_home()?
        .search("Kings")?
        .context("book not found")?;
    println!("{}'s {}", book.author().unwrap(), book.name());

    let (term_w, term_h) = crossterm::terminal::size()?;

    let mut w = std::io::stdout();
    let mut display = Display::new(book, 80, term_w, term_h);

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
            break Ok(event);
        }
    }
}
