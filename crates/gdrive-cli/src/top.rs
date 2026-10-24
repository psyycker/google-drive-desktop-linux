//! `gdrive top`: a live, full-screen view of what the daemon is doing, like the
//! window's activity view: transfers with progress, recent changes, problems.

use std::io::{Stdout, Write};
use std::time::Duration;

use anyhow::Result;
use chrono::{Local, Utc};
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, queue};
use gdrive_core::ipc::{self, Request, Response};
use gdrive_core::status::{human_bytes, Activity, ActivityKind, Direction, Status, SyncState};

use crate::picker::Screen;

const REFRESH: Duration = Duration::from_secs(1);

/// Which recent activity is listed.
#[derive(Clone, Copy, PartialEq)]
enum Filter {
    All,
    Downloaded,
    Uploaded,
    Removed,
    Conflicts,
}

impl Filter {
    fn next(self) -> Self {
        match self {
            Filter::All => Filter::Downloaded,
            Filter::Downloaded => Filter::Uploaded,
            Filter::Uploaded => Filter::Removed,
            Filter::Removed => Filter::Conflicts,
            Filter::Conflicts => Filter::All,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Filter::All => "everything",
            Filter::Downloaded => "downloads",
            Filter::Uploaded => "uploads",
            Filter::Removed => "removals",
            Filter::Conflicts => "conflicts",
        }
    }

    fn keeps(self, kind: ActivityKind) -> bool {
        match self {
            Filter::All => true,
            Filter::Downloaded => matches!(kind, ActivityKind::Downloaded | ActivityKind::CreatedFolderLocal),
            Filter::Uploaded => matches!(kind, ActivityKind::Uploaded | ActivityKind::CreatedFolderRemote),
            Filter::Removed => matches!(kind, ActivityKind::DeletedLocal | ActivityKind::DeletedRemote),
            Filter::Conflicts => kind == ActivityKind::Conflict,
        }
    }
}

/// The same wording as the window's activity list.
fn verb(kind: ActivityKind) -> (&'static str, &'static str, Color) {
    match kind {
        ActivityKind::Uploaded => ("↑", "Uploaded", Color::Blue),
        ActivityKind::Downloaded => ("↓", "Downloaded", Color::Green),
        ActivityKind::CreatedFolderLocal => ("+", "Folder created here", Color::Green),
        ActivityKind::CreatedFolderRemote => ("+", "Folder created in Drive", Color::Blue),
        ActivityKind::MovedLocal => ("→", "Moved here", Color::Reset),
        ActivityKind::MovedRemote => ("→", "Moved in Drive", Color::Reset),
        ActivityKind::DeletedLocal => ("×", "Removed here", Color::Reset),
        ActivityKind::DeletedRemote => ("×", "Removed from Drive", Color::Reset),
        ActivityKind::Conflict => ("!", "Conflict, both kept", Color::Yellow),
    }
}

fn state_text(st: &Status) -> (&'static str, Color) {
    match st.state {
        SyncState::SetupRequired => ("Setup required: run `gdrive setup`", Color::Yellow),
        SyncState::SignedOut => ("Signed out: run `gdrive setup`", Color::Yellow),
        SyncState::SigningIn => ("Waiting for the browser sign-in", Color::Yellow),
        SyncState::Starting => ("Starting", Color::Reset),
        SyncState::Idle => ("Up to date", Color::Green),
        SyncState::Syncing => ("Syncing", Color::Blue),
        SyncState::Paused => ("Paused", Color::Yellow),
        SyncState::Offline => ("Offline, retrying", Color::Yellow),
        SyncState::Error => ("Error", Color::Red),
    }
}

/// `width` characters at most; long paths keep their end (the file name).
fn fit_left(s: &str, width: usize) -> String {
    let n = s.chars().count();
    if n <= width {
        return s.to_owned();
    }
    if width == 0 {
        return String::new();
    }
    let tail: String = s.chars().skip(n - (width - 1)).collect();
    format!("…{tail}")
}

fn fit(s: &str, width: usize) -> String {
    if s.chars().count() <= width {
        s.to_owned()
    } else {
        s.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
    }
}

fn bar(done: u64, total: u64, width: usize) -> String {
    if total == 0 {
        return format!("[{}]", "·".repeat(width));
    }
    let filled = ((done.min(total) as u128 * width as u128) / total as u128) as usize;
    format!("[{}{}]", "█".repeat(filled), "░".repeat(width - filled))
}

/// Counts of today's activity, from the recent list.
fn today(recent: &[&Activity]) -> String {
    let date = Local::now().date_naive();
    // Files only: a folder created doesn't count as a download.
    let of = |kinds: &[ActivityKind]| {
        recent.iter().filter(|a| a.time.with_timezone(&Local).date_naive() == date && kinds.contains(&a.kind)).count()
    };
    let removed: &[ActivityKind] = &[ActivityKind::DeletedLocal, ActivityKind::DeletedRemote];
    let parts: Vec<String> = [
        (&[ActivityKind::Downloaded][..], "downloaded"),
        (&[ActivityKind::Uploaded][..], "uploaded"),
        (removed, "removed"),
    ]
    .into_iter()
    .filter_map(|(kinds, word)| match of(kinds) {
            0 => None,
            n => Some(format!("{n} {word}")),
        })
        .collect();
    if parts.is_empty() {
        "nothing yet today".into()
    } else {
        format!("today: {}", parts.join(" · "))
    }
}

fn ago(time: chrono::DateTime<Utc>) -> String {
    let secs = (Utc::now() - time).num_seconds().max(0);
    match secs {
        0..=59 => "just now".into(),
        60..=3599 => format!("{} min ago", secs / 60),
        _ => time.with_timezone(&Local).format("%H:%M").to_string(),
    }
}

struct View {
    filter: Filter,
    scroll: usize,
}

/// One screen line: text with an optional colour and attribute.
struct Line {
    text: String,
    color: Color,
    attr: Attribute,
}

fn line(text: impl Into<String>) -> Line {
    Line { text: text.into(), color: Color::Reset, attr: Attribute::Reset }
}

fn colored(text: impl Into<String>, color: Color) -> Line {
    Line { text: text.into(), color, attr: Attribute::Reset }
}

fn heading(text: impl Into<String>) -> Line {
    Line { text: text.into(), color: Color::Reset, attr: Attribute::Bold }
}

impl View {
    fn lines(&mut self, st: Option<&Status>, problem: Option<&str>, width: usize, height: usize) -> Vec<Line> {
        let mut out = Vec::new();
        let now = Local::now().format("%H:%M:%S").to_string();
        let who = st.and_then(|s| s.account.as_ref()).map(|a| format!(" · {}", a.email)).unwrap_or_default();
        let title = format!(" gdrive top{who}");
        let pad = width.saturating_sub(title.chars().count() + now.len() + 1);
        out.push(Line { text: format!("{title}{}{now} ", " ".repeat(pad)), color: Color::Reset, attr: Attribute::Reverse });

        let Some(st) = st else {
            out.push(line(""));
            out.push(colored(format!(" Can't reach gdrived: {}", problem.unwrap_or("not running")), Color::Red));
            out.push(line(" Start it with `gdrive setup`; this view keeps trying."));
            return out;
        };

        // Overview, like the window's header and banner.
        let (state, color) = state_text(st);
        let mut overview = format!(" ● {state}");
        if let Some(speed) = st.speed_text() {
            overview += &format!("   {speed}");
        }
        if st.pending > 0 {
            overview += &format!("   {} waiting", st.pending);
        }
        if let Some(t) = st.last_synced {
            overview += &format!("   last synced {}", ago(t));
        }
        out.push(colored(overview, color));
        if let Some(msg) = &st.message {
            out.push(colored(format!("   {msg}"), color));
        }
        let mode = match st.selected_folders {
            None => "Mirror: whole Drive".to_owned(),
            Some(0) => "Stream: no folders chosen (`gdrive folders browse`)".to_owned(),
            Some(n) => format!("Stream: {n} folder(s)"),
        };
        let storage = match &st.quota {
            Some(q) => match q.limit {
                Some(limit) => format!(
                    " Storage {} {} of {}",
                    bar(q.used, limit, 12),
                    human_bytes(q.used),
                    human_bytes(limit)
                ),
                None => format!(" Storage {} used", human_bytes(q.used)),
            },
            None => " Storage —".to_owned(),
        };
        out.push(line(format!("{storage}   {mode} → {}", st.sync_root)));
        if let Some(r) = &st.relocation {
            let to = r.to.as_deref().unwrap_or("main drive");
            out.push(line(format!(" Moving {} → {to}  {}", r.folder, bar(r.bytes_done, r.bytes_total, 20))));
        }
        if let Some(u) = &st.update {
            out.push(colored(format!(" Version {} is out: `gdrive update`", u.latest), Color::Yellow));
        }
        out.push(line(""));

        // In progress.
        let footer = 1;
        let budget = height.saturating_sub(out.len() + footer);
        if !st.transfers.is_empty() {
            out.push(heading(format!(" IN PROGRESS ({})", st.transfers.len())));
            let room = (budget / 3).max(3);
            let shown = st.transfers.len().min(room);
            for t in st.transfers.iter().take(if shown < st.transfers.len() { shown - 1 } else { shown }) {
                let (arrow, color) = match t.direction {
                    Direction::Download => ("↓", Color::Green),
                    Direction::Upload => ("↑", Color::Blue),
                };
                let size = match (t.bytes_done * 100).checked_div(t.bytes_total) {
                    Some(pct) => format!(
                        "{pct:>3}%  {:>9} of {:<9}",
                        human_bytes(t.bytes_done),
                        human_bytes(t.bytes_total)
                    ),
                    None => format!("      {:>9}", human_bytes(t.bytes_done)),
                };
                let head = format!(" {arrow} {} {size}  ", bar(t.bytes_done, t.bytes_total, 16));
                let path = fit_left(&t.path, width.saturating_sub(head.chars().count()));
                out.push(colored(format!("{head}{path}"), color));
            }
            if shown < st.transfers.len() {
                out.push(line(format!("   … and {} more", st.transfers.len() - shown + 1)));
            }
            out.push(line(""));
        }

        // Problems.
        if !st.errors.is_empty() {
            out.push(heading(format!(" PROBLEMS ({})", st.errors.len())));
            for e in st.errors.iter().take(3) {
                let path = if e.path.is_empty() { "—" } else { &e.path };
                out.push(colored(format!(" ! {}  {}  {}", ago(e.time), path, e.message), Color::Red));
            }
            if st.errors.len() > 3 {
                out.push(line(format!("   … and {} more (`gdrive status` lists them)", st.errors.len() - 3)));
            }
            out.push(line(""));
        }

        // Recent activity, scrollable.
        let recent: Vec<&Activity> = st.recent.iter().collect();
        let shown: Vec<&&Activity> = recent.iter().filter(|a| self.filter.keeps(a.kind)).collect();
        out.push(heading(format!(" RECENT · {} · {}", self.filter.label(), today(&recent))));
        let room = height.saturating_sub(out.len() + footer);
        self.scroll = self.scroll.min(shown.len().saturating_sub(room));
        if shown.is_empty() {
            out.push(line(match self.filter {
                Filter::All => "   Nothing yet: changes in your Drive folder show up here.".to_owned(),
                f => format!("   No {} in the recent activity.", f.label()),
            }));
        }
        for a in shown.iter().skip(self.scroll).take(room) {
            let (sym, word, color) = verb(a.kind);
            let when = a.time.with_timezone(&Local).format("%H:%M:%S");
            let detail = a.detail.as_deref().map(|d| format!("  ({d})")).unwrap_or_default();
            let head = format!(" {when}  {sym} {word:<24}");
            let rest = fit_left(&format!("{}{detail}", a.path), width.saturating_sub(head.chars().count()));
            out.push(colored(format!("{head}{rest}"), color));
        }
        out
    }

    fn draw(&mut self, out: &mut Stdout, st: Option<&Status>, problem: Option<&str>) -> Result<()> {
        let (width, height) = terminal::size()?;
        let (width, height) = (width as usize, height as usize);
        let lines = self.lines(st, problem, width, height);
        queue!(out, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;
        for (i, l) in lines.iter().take(height.saturating_sub(1)).enumerate() {
            queue!(
                out,
                cursor::MoveTo(0, i as u16),
                SetAttribute(l.attr),
                SetForegroundColor(l.color),
                Print(fit(&l.text, width)),
                ResetColor,
                SetAttribute(Attribute::Reset)
            )?;
        }
        let paused = st.is_some_and(|s| s.state == SyncState::Paused);
        let keys = format!(
            " q quit   {}   s sync now   f show {}   ↑↓ scroll",
            if paused { "p resume" } else { "p pause" },
            self.filter.next().label()
        );
        queue!(
            out,
            cursor::MoveTo(0, height.saturating_sub(1) as u16),
            SetAttribute(Attribute::Dim),
            Print(fit(&keys, width)),
            SetAttribute(Attribute::Reset)
        )?;
        out.flush()?;
        Ok(())
    }
}

async fn fetch() -> Result<Status, String> {
    match ipc::call(&Request::Status).await {
        Ok(Response::Status { status }) => Ok(*status),
        Ok(_) => Err("unexpected reply".into()),
        Err(e) => Err(format!("{e:#}").split(';').next().unwrap_or_default().to_owned()),
    }
}

/// A key pressed within `timeout`, if any.
fn key_within(timeout: Duration) -> Result<Option<event::KeyEvent>> {
    if event::poll(timeout)? {
        if let Event::Key(key) = event::read()? {
            if key.kind != KeyEventKind::Release {
                return Ok(Some(key));
            }
        }
        // A resize or other event: redraw now.
        return Ok(Some(event::KeyEvent::from(KeyCode::Null)));
    }
    Ok(None)
}

pub async fn run() -> Result<()> {
    let mut view = View { filter: Filter::All, scroll: 0 };
    let mut screen = Screen::enter()?;
    let mut status = fetch().await;
    loop {
        let (st, problem) = match &status {
            Ok(s) => (Some(s), None),
            Err(e) => (None, Some(e.as_str())),
        };
        view.draw(&mut screen.0, st, problem)?;
        let key = tokio::task::spawn_blocking(|| key_within(REFRESH)).await??;
        if let Some(key) = key {
            match key.code {
                KeyCode::Char('q') | KeyCode::Esc => break,
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                KeyCode::Char('p') => {
                    let paused = status.as_ref().is_ok_and(|s| s.state == SyncState::Paused);
                    let _ = ipc::call(if paused { &Request::Resume } else { &Request::Pause }).await;
                }
                KeyCode::Char('s') => {
                    let _ = ipc::call(&Request::SyncNow).await;
                }
                KeyCode::Char('f') => {
                    view.filter = view.filter.next();
                    view.scroll = 0;
                }
                KeyCode::Up | KeyCode::Char('k') => view.scroll = view.scroll.saturating_sub(1),
                KeyCode::Down | KeyCode::Char('j') => view.scroll += 1,
                KeyCode::PageUp => view.scroll = view.scroll.saturating_sub(10),
                KeyCode::PageDown => view.scroll += 10,
                KeyCode::Home => view.scroll = 0,
                _ => {}
            }
        }
        status = fetch().await;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_paths_keep_the_file_name() {
        assert_eq!(fit_left("Work/Reports/2024/q3.pdf", 10), "…24/q3.pdf");
        assert_eq!(fit_left("a.txt", 10), "a.txt");
        assert_eq!(bar(5, 10, 4), "[██░░]");
        assert_eq!(bar(0, 0, 3), "[···]");
        assert_eq!(bar(20, 10, 2), "[██]");
    }

    #[test]
    fn filters_match_the_kinds_they_name() {
        assert!(Filter::Downloaded.keeps(ActivityKind::Downloaded));
        assert!(!Filter::Downloaded.keeps(ActivityKind::Uploaded));
        assert!(Filter::Removed.keeps(ActivityKind::DeletedRemote));
        assert!(Filter::All.keeps(ActivityKind::Conflict));
        let mut f = Filter::All;
        for _ in 0..5 {
            f = f.next();
        }
        assert!(f == Filter::All);
    }
}
