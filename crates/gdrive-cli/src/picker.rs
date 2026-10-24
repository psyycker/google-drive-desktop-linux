//! Full-screen folder picker: the Drive as a tree, opened with the arrow keys and
//! ticked with Space. Folders are listed from the daemon as they are opened.

use std::collections::HashMap;
use std::io::{Stdout, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::style::{Attribute, Print, SetAttribute};
use crossterm::terminal::{self, ClearType};
use crossterm::{cursor, execute, queue};
use gdrive_core::ipc::{self, Request, Response};

/// What to sync: `None` is the whole Drive.
pub type Selection = Option<Vec<PathBuf>>;

/// Raw mode and the alternate screen, undone on drop (also on errors and panics
/// unwinding through the picker).
struct Screen(Stdout);

impl Screen {
    fn enter() -> Result<Self> {
        terminal::enable_raw_mode()?;
        let mut out = std::io::stdout();
        execute!(out, terminal::EnterAlternateScreen, cursor::Hide)?;
        Ok(Self(out))
    }
}

impl Drop for Screen {
    fn drop(&mut self) {
        let _ = execute!(self.0, cursor::Show, terminal::LeaveAlternateScreen);
        let _ = terminal::disable_raw_mode();
    }
}

enum Children {
    Loaded(Vec<PathBuf>),
    Failed(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Mark {
    Chosen,
    /// Inside a chosen folder (or the whole Drive is synced).
    Included,
    /// Something inside it is chosen.
    Partly,
    No,
}

/// One line of the tree as shown.
struct Row {
    path: PathBuf,
    depth: usize,
    /// For notes like "Loading…" or an error under an opened folder.
    note: Option<String>,
}

struct Picker {
    children: HashMap<PathBuf, Children>,
    open: Vec<PathBuf>,
    /// The ticked folders, kept even while the whole Drive is chosen, to switch back.
    chosen: Vec<PathBuf>,
    whole: bool,
    saved: Selection,
    cursor: usize,
    top: usize,
    message: Option<String>,
    confirming: Option<String>,
}

async fn list(path: &Path) -> Result<Vec<PathBuf>> {
    let Response::Listing { folders } = ipc::call(&Request::Browse { path: path.to_owned() }).await? else {
        bail!("unexpected reply")
    };
    Ok(folders.into_iter().map(|f| f.path).collect())
}

fn name(path: &Path) -> String {
    path.file_name().unwrap_or_default().to_string_lossy().into_owned()
}

impl Picker {
    fn mark(&self, path: &Path) -> Mark {
        if self.whole || self.chosen.iter().any(|c| path.starts_with(c) && c != path) {
            Mark::Included
        } else if self.chosen.iter().any(|c| c == path) {
            Mark::Chosen
        } else if self.chosen.iter().any(|c| c.starts_with(path)) {
            Mark::Partly
        } else {
            Mark::No
        }
    }

    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        self.push_rows(Path::new(""), 0, &mut rows);
        rows
    }

    fn push_rows(&self, parent: &Path, depth: usize, rows: &mut Vec<Row>) {
        let note = |text: &str| Row { path: parent.to_owned(), depth, note: Some(text.to_owned()) };
        match self.children.get(parent) {
            None => rows.push(note("Loading…")),
            Some(Children::Failed(e)) => rows.push(note(e)),
            Some(Children::Loaded(kids)) if kids.is_empty() => {
                rows.push(note(if depth == 0 { "Your Drive has no folders." } else { "No folders inside" }))
            }
            Some(Children::Loaded(kids)) => {
                for kid in kids {
                    rows.push(Row { path: kid.clone(), depth, note: None });
                    if self.open.contains(kid) {
                        self.push_rows(kid, depth + 1, rows);
                    }
                }
            }
        }
    }

    async fn load(&mut self, path: &Path) {
        if self.children.contains_key(path) {
            return;
        }
        let kids = match list(path).await {
            Ok(kids) => Children::Loaded(kids),
            Err(e) => Children::Failed(format!("{e:#}")),
        };
        self.children.insert(path.to_owned(), kids);
    }

    fn selection(&self) -> Selection {
        (!self.whole).then(|| {
            let mut c = self.chosen.clone();
            c.sort();
            c
        })
    }

    /// What applying the selection would take off this computer.
    fn removals(&self) -> Option<String> {
        let new = self.selection();
        match (&self.saved, &new) {
            (_, None) => None,
            (None, Some(_)) => Some("everything except the chosen folders".into()),
            (Some(old), Some(new)) => {
                let gone: Vec<String> = old
                    .iter()
                    .filter(|o| !new.iter().any(|n| o.starts_with(n)))
                    .map(|o| o.display().to_string())
                    .collect();
                (!gone.is_empty()).then(|| gone.join(", "))
            }
        }
    }

    fn toggle(&mut self, path: &Path) {
        if self.whole {
            self.message = Some("The whole Drive is chosen. Press a to choose folders instead.".into());
            return;
        }
        if let Some(parent) = self.chosen.iter().find(|c| path.starts_with(c) && *c != path) {
            self.message = Some(format!("Already included with {}.", parent.display()));
            return;
        }
        if let Some(pos) = self.chosen.iter().position(|c| c == path) {
            self.chosen.remove(pos);
        } else {
            // Folders inside it come along anyway.
            self.chosen.retain(|c| !c.starts_with(path));
            self.chosen.push(path.to_owned());
        }
    }

    fn draw(&mut self, out: &mut Stdout, rows: &[Row]) -> Result<()> {
        let (width, height) = terminal::size()?;
        let (width, height) = (width as usize, height as usize);
        // Header 3 lines, footer 3 lines.
        let view = height.saturating_sub(6).max(1);
        if self.cursor < self.top {
            self.top = self.cursor;
        } else if self.cursor >= self.top + view {
            self.top = self.cursor + 1 - view;
        }
        let fit = |s: String| -> String {
            if s.chars().count() <= width {
                s
            } else {
                s.chars().take(width.saturating_sub(1)).collect::<String>() + "…"
            }
        };
        queue!(out, cursor::MoveTo(0, 0), terminal::Clear(ClearType::All))?;
        queue!(out, SetAttribute(Attribute::Bold), Print(fit("Choose the Drive folders to keep on this computer".into())))?;
        queue!(out, SetAttribute(Attribute::Reset), cursor::MoveTo(0, 1))?;
        let summary = match self.selection() {
            None => "Syncing: your whole Drive".to_owned(),
            Some(c) if c.is_empty() => "Syncing: nothing chosen yet".to_owned(),
            Some(c) => format!("Syncing: {}", c.iter().map(|p| p.display().to_string()).collect::<Vec<_>>().join(", ")),
        };
        queue!(out, Print(fit(summary)))?;

        for (i, row) in rows.iter().enumerate().skip(self.top).take(view) {
            queue!(out, cursor::MoveTo(0, (3 + i - self.top) as u16))?;
            let indent = "    ".repeat(row.depth);
            let line = match &row.note {
                Some(note) => format!("    {indent}    {note}"),
                None => {
                    let open = self.open.contains(&row.path);
                    let arrow = match self.children.get(&row.path) {
                        Some(Children::Loaded(k)) if k.is_empty() => " ",
                        _ if open => "▾",
                        _ => "▸",
                    };
                    let mark = match self.mark(&row.path) {
                        Mark::Chosen => "[x]",
                        Mark::Included => "[·]",
                        Mark::Partly => "[~]",
                        Mark::No => "[ ]",
                    };
                    format!("  {indent}{arrow} {mark} {}", name(&row.path))
                }
            };
            if i == self.cursor {
                queue!(out, SetAttribute(Attribute::Reverse), Print(fit(format!("{line:<width$}"))), SetAttribute(Attribute::Reset))?;
            } else if row.note.is_some() || self.mark(&row.path) == Mark::Included {
                queue!(out, SetAttribute(Attribute::Dim), Print(fit(line)), SetAttribute(Attribute::Reset))?;
            } else {
                queue!(out, Print(fit(line)))?;
            }
        }

        let foot = height.saturating_sub(3) as u16;
        queue!(out, cursor::MoveTo(0, foot))?;
        if let Some(what) = &self.confirming {
            queue!(
                out,
                SetAttribute(Attribute::Bold),
                Print(fit(format!("Remove {what} from this computer? It stays on Drive."))),
                SetAttribute(Attribute::Reset),
                cursor::MoveTo(0, foot + 1),
                Print(fit("y: yes, apply   any other key: go back".into()))
            )?;
        } else {
            if let Some(m) = &self.message {
                queue!(out, Print(fit(m.clone())))?;
            }
            queue!(
                out,
                cursor::MoveTo(0, foot + 1),
                Print(fit("↑↓ move   → open   ← close   Space choose   a whole Drive on/off".into())),
                cursor::MoveTo(0, foot + 2),
                Print(fit("Enter save   Esc cancel      [x] chosen  [~] some inside  [·] included".into()))
            )?;
        }
        out.flush()?;
        Ok(())
    }
}

async fn next_key() -> Result<KeyEvent> {
    loop {
        let ev = tokio::task::spawn_blocking(event::read).await??;
        if let Event::Key(key) = ev {
            if key.kind != KeyEventKind::Release {
                return Ok(key);
            }
        }
        // Resizes and the like: redraw by returning a no-op key.
        if let Event::Resize(..) = ev {
            return Ok(KeyEvent::from(KeyCode::Null));
        }
    }
}

/// Lets the user choose folders, starting from `saved`. Returns `None` when cancelled.
pub async fn pick(saved: Selection) -> Result<Option<Selection>> {
    let mut p = Picker {
        children: HashMap::new(),
        open: Vec::new(),
        chosen: saved.clone().unwrap_or_default(),
        whole: saved.is_none(),
        saved,
        cursor: 0,
        top: 0,
        message: None,
        confirming: None,
    };
    // Opens the folders on the way to each chosen one, so they can be seen.
    p.load(Path::new("")).await;
    for c in p.chosen.clone() {
        for a in c.ancestors().skip(1).filter(|a| !a.as_os_str().is_empty()) {
            if !p.open.contains(&a.to_path_buf()) {
                p.load(a).await;
                p.open.push(a.to_path_buf());
            }
        }
    }

    let mut screen = Screen::enter()?;
    loop {
        let rows = p.rows();
        p.cursor = p.cursor.min(rows.len().saturating_sub(1));
        p.draw(&mut screen.0, &rows)?;
        let key = next_key().await?;
        if p.confirming.take().is_some() {
            if matches!(key.code, KeyCode::Char('y' | 'Y')) {
                return Ok(Some(p.selection()));
            }
            continue;
        }
        p.message = None;
        let row = rows.get(p.cursor);
        let folder = row.filter(|r| r.note.is_none()).map(|r| r.path.clone());
        match key.code {
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(None),
            KeyCode::Esc | KeyCode::Char('q') => return Ok(None),
            KeyCode::Up | KeyCode::Char('k') => p.cursor = p.cursor.saturating_sub(1),
            KeyCode::Down | KeyCode::Char('j') => p.cursor = (p.cursor + 1).min(rows.len().saturating_sub(1)),
            KeyCode::PageUp => p.cursor = p.cursor.saturating_sub(10),
            KeyCode::PageDown => p.cursor = (p.cursor + 10).min(rows.len().saturating_sub(1)),
            KeyCode::Home => p.cursor = 0,
            KeyCode::End => p.cursor = rows.len().saturating_sub(1),
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(f) = folder {
                    if !p.open.contains(&f) {
                        p.open.push(f.clone());
                        // Shows "Loading…" while the daemon lists it.
                        let rows = p.rows();
                        p.draw(&mut screen.0, &rows)?;
                        p.load(&f).await;
                    } else if let Some(Children::Loaded(kids)) = p.children.get(&f) {
                        // Already open: step into it.
                        if !kids.is_empty() {
                            p.cursor += 1;
                        }
                    }
                }
            }
            KeyCode::Left | KeyCode::Char('h') | KeyCode::Backspace => {
                let Some(r) = row else { continue };
                if r.note.is_none() && p.open.contains(&r.path) {
                    p.open.retain(|o| !o.starts_with(&r.path));
                } else {
                    // Go to the parent row, and close it.
                    let parent = if r.note.is_some() { r.path.clone() } else { r.path.parent().unwrap_or(Path::new("")).to_owned() };
                    if let Some(i) = rows.iter().position(|x| x.note.is_none() && x.path == parent) {
                        p.cursor = i;
                        p.open.retain(|o| !o.starts_with(&parent));
                    }
                }
            }
            KeyCode::Char(' ') => {
                if let Some(f) = folder {
                    p.toggle(&f);
                }
            }
            KeyCode::Char('a') => p.whole = !p.whole,
            KeyCode::Enter => {
                if p.selection() == p.saved {
                    return Ok(Some(p.selection()));
                }
                match p.removals() {
                    Some(what) => p.confirming = Some(what),
                    None => return Ok(Some(p.selection())),
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picker(chosen: &[&str], saved: Selection) -> Picker {
        Picker {
            children: HashMap::new(),
            open: Vec::new(),
            chosen: chosen.iter().map(PathBuf::from).collect(),
            whole: false,
            saved,
            cursor: 0,
            top: 0,
            message: None,
            confirming: None,
        }
    }

    #[test]
    fn marks_and_toggles_like_the_window() {
        let mut p = picker(&["Work/Reports"], Some(vec![]));
        assert!(p.mark(Path::new("Work")) == Mark::Partly);
        assert!(p.mark(Path::new("Work/Reports")) == Mark::Chosen);
        assert!(p.mark(Path::new("Work/Reports/2024")) == Mark::Included);
        assert!(p.mark(Path::new("Workshop")) == Mark::No);
        // Choosing the parent takes over the folders inside it.
        p.toggle(Path::new("Work"));
        assert_eq!(p.selection(), Some(vec![PathBuf::from("Work")]));
        // A folder inside a chosen one can't be unticked on its own.
        p.toggle(Path::new("Work/Reports"));
        assert_eq!(p.selection(), Some(vec![PathBuf::from("Work")]));
        p.toggle(Path::new("Work"));
        assert_eq!(p.selection(), Some(vec![]));
    }

    #[test]
    fn asks_before_removing_local_copies() {
        let saved = Some(vec![PathBuf::from("Work"), PathBuf::from("Photos")]);
        let mut p = picker(&["Work", "Photos"], saved.clone());
        assert_eq!(p.removals(), None);
        p.toggle(Path::new("Photos"));
        assert_eq!(p.removals().as_deref(), Some("Photos"));
        p.whole = true;
        assert_eq!(p.removals(), None);
        let p = picker(&["Work"], None);
        assert_eq!(p.removals().as_deref(), Some("everything except the chosen folders"));
    }
}
