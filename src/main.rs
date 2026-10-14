mod ago;
mod herdr;
mod parse;
mod record;
mod registry;
mod timeline;
mod transcript;
mod ui;

use std::env;
use std::io;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use chrono::Utc;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use ratatui::DefaultTerminal;

use crate::registry::Store;
use crate::timeline::{App, Feeds};

const REFRESH: Duration = Duration::from_secs(2);

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None => run_tui(),
        Some("record") => {
            record::run();
            ExitCode::SUCCESS
        }
        Some(_) => {
            eprintln!("usage: herdr-skill-timeline [record]");
            ExitCode::from(2)
        }
    }
}

fn run_tui() -> ExitCode {
    let in_herdr = env::var("HERDR_ENV").is_ok_and(|v| v == "1");
    let own_pane = env::var("HERDR_PANE_ID").ok().filter(|p| !p.is_empty());
    let (true, Some(own_pane)) = (in_herdr, own_pane) else {
        eprintln!("herdr-skill-timeline: not running inside a herdr pane");
        return ExitCode::FAILURE;
    };

    let mut app = App::new(
        own_pane,
        Store::default_location(),
        Feeds::new(transcript::projects_dir()),
    );
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut app);
    ratatui::restore();
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("herdr-skill-timeline: {e}");
            ExitCode::FAILURE
        }
    }
}

fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> io::Result<()> {
    let mut scroll: u16 = 0;
    let mut view = app.refresh(herdr::fetch_snapshot(), Utc::now());
    let mut last_refresh = Instant::now();
    loop {
        terminal.draw(|frame| ui::render(frame, &view, Utc::now(), &mut scroll))?;

        // Wait for a key/resize, or until the next refresh is due.
        if event::poll(REFRESH.saturating_sub(last_refresh.elapsed()))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(());
                }
                KeyCode::Char('j') | KeyCode::Down => scroll = scroll.saturating_add(1),
                KeyCode::Char('k') | KeyCode::Up => scroll = scroll.saturating_sub(1),
                _ => {}
            }
        }

        if last_refresh.elapsed() >= REFRESH {
            view = app.refresh(herdr::fetch_snapshot(), Utc::now());
            last_refresh = Instant::now();
        }
    }
}
