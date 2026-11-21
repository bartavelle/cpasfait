//! Rendering.

use crate::app::{App, Mode};
use crate::keymap::HELP;
use crate::search::Search;
use crate::search::short_id;
use crate::timebox::{self, TimePeriod};
use cpasfait_core::dates::today_local;
use cpasfait_core::report::fmt_duration;
use cpasfait_core::score::Score;
use cpasfait_core::{DepInfo, DepState, Session, Task};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, HighlightSpacing, List, ListItem, ListState, Paragraph, Wrap};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn render(f: &mut Frame, app: &mut App) {
    let [tabs, body, status] =
        Layout::vertical([Constraint::Length(1), Constraint::Min(1), Constraint::Length(1)]).areas(f.area());
    render_tabs(f, app, tabs);
    let show_detail = app.show_detail && body.width >= 70;
    let (list_area, detail_area) = if show_detail {
        let [l, d] = Layout::horizontal([Constraint::Percentage(60), Constraint::Percentage(40)]).areas(body);
        (l, Some(d))
    } else {
        (body, None)
    };
    let mut state = std::mem::take(&mut app.list);
    app.page_height = render_tree(f, app, &mut state, list_area);
    app.list = state;
    if let Some(d) = detail_area {
        render_detail(f, app, d);
    }
    render_status(f, app, status);
    render_time_box(f, app, body);
    if let Mode::Search(search) = &app.mode {
        render_search(f, search, body);
    }
    if let Mode::Help { scroll, page } = &mut app.mode {
        render_help(f, scroll, page);
    }
}

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mut spans = vec![];
    for (i, t) in app.tabs.iter().enumerate() {
        let name = if t.name.is_empty() { "(unnamed)" } else { &t.name };
        let marks = match (&t.remote, &t.pending_move) {
            (_, Some(_)) => " ⇅↪", // moved elsewhere: :tabaccept
            (Some(_), None) => " ⇅",
            (None, None) => "",
        };
        let label = format!(" {}:{}{marks} ", i + 1, name);
        let style = if i == app.tab_idx {
            Style::new().bg(Color::Blue).fg(Color::White).bold()
        } else {
            Style::new().fg(Color::Gray)
        };
        spans.push(Span::styled(label, style));
        spans.push(Span::raw(" "));
    }
    let user = format!("{} ", app.repo.user());
    let left = Line::from(spans);
    f.render_widget(Paragraph::new(left), area);
    let w = user.width() as u16;
    if area.width > w {
        let r = Rect {
            x: area.x + area.width - w,
            width: w,
            ..area
        };
        f.render_widget(Paragraph::new(user.dim()), r);
    }
}

fn due_style(due: jiff::civil::Date) -> Style {
    let today = today_local();
    if due < today {
        Style::new().fg(Color::Red).bold()
    } else if due == today {
        Style::new().fg(Color::Yellow).bold()
    } else {
        Style::new().fg(Color::Yellow)
    }
}

/// Spans shown after the title: priority, due, tags, who is working on it.
fn task_suffix<'a>(app: &App, t: &'a Task) -> Vec<Span<'a>> {
    let mut v = vec![];
    if !t.is_done() && app.tree.is_blocked(t.id) {
        v.push(Span::styled("  ⊘blocked", Style::new().fg(Color::Red)));
    }
    if let Some(d) = t.due {
        v.push(Span::styled(format!("  ⌛{d}"), due_style(d)));
    }
    for tag in &t.tags {
        v.push(Span::styled(format!(" #{tag}"), Style::new().fg(Color::Cyan)));
    }
    for s in app.open_sessions.iter().filter(|s| s.task == Some(t.id)) {
        if Some(s.id) == app.active.as_ref().map(|a| a.id) {
            v.push(Span::styled("  ⏱", Style::new().fg(Color::Green).bold()));
        } else {
            v.push(Span::styled(format!("  ⏱{}", s.user), Style::new().fg(Color::Magenta)));
        }
    }
    v
}

fn task_prefix(app: &App, t: &Task, depth: usize, has_children: bool) -> Vec<Span<'static>> {
    let fold = if !has_children {
        "  "
    } else if app.collapsed.contains(&t.id) {
        "▸ "
    } else {
        "▾ "
    };
    let check = if t.is_done() { "[x] " } else { "[ ] " };
    let mut v = vec![Span::raw("  ".repeat(depth)), Span::raw(fold).dim(), Span::raw(check)];
    if t.priority > 0 {
        let color = match t.priority {
            1 => Color::Blue,
            2 => Color::Yellow,
            _ => Color::Red,
        };
        v.push(Span::styled(
            format!("{} ", "!".repeat(t.priority.min(3) as usize)),
            Style::new().fg(color).bold(),
        ));
    }
    v
}

/// Marks the selected row (the list reserves its width on every row).
const SELECTED_SYMBOL: &str = "▶ ";

fn render_tree(f: &mut Frame, app: &App, state: &mut ListState, area: Rect) -> usize {
    let title = match app.tab() {
        Some(t) => format!(" {} ", if t.name.is_empty() { "(unnamed)" } else { &t.name }),
        None => " no tab ".into(),
    };
    let block = Block::new().borders(Borders::RIGHT).title(title.bold());
    let inner = block.inner(area);
    let editing = match &app.mode {
        Mode::Insert { task, editor, .. } => Some((*task, editor.clone())),
        _ => None,
    };
    let mut cursor_pos = None;
    let items: Vec<ListItem> = app
        .rows
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let t = app.tree.get(row.id).expect("row of a visible task");
            let mut spans = task_prefix(app, t, row.depth, row.has_children);
            let prefix_w: usize = spans.iter().map(|s| s.content.width()).sum();
            match &editing {
                Some((id, ed)) if *id == row.id => {
                    spans.push(Span::styled(ed.text().to_owned(), Style::new().underlined()));
                    cursor_pos = Some((i, SELECTED_SYMBOL.width() + prefix_w + ed.cursor_col()));
                }
                _ => {
                    let mut st = Style::new();
                    if t.is_done() {
                        st = st.dim().crossed_out();
                    }
                    if app.next == Some(t.id) {
                        st = st.bold();
                    }
                    let title = if t.title.is_empty() {
                        "(untitled)".to_owned()
                    } else {
                        t.title.clone()
                    };
                    spans.push(Span::styled(title, st));
                    spans.extend(task_suffix(app, t));
                }
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    if items.is_empty() {
        f.render_widget(block, area);
        f.render_widget(Paragraph::new("No tasks. Press n to add one, ? for help.").dim(), inner);
        return inner.height as usize;
    }
    let list = List::new(items)
        .block(block)
        .highlight_symbol(Line::from(SELECTED_SYMBOL).fg(Color::Yellow).bold())
        .highlight_spacing(HighlightSpacing::Always)
        .highlight_style(Style::new().add_modifier(Modifier::REVERSED));
    f.render_stateful_widget(list, area, state);
    if let Some((i, col)) = cursor_pos {
        let y = i.saturating_sub(state.offset());
        if y < inner.height as usize {
            f.set_cursor_position(Position::new(inner.x + col as u16, inner.y + y as u16));
        }
    }
    inner.height as usize
}

fn fmt_time(ms: i64) -> String {
    match jiff::Timestamp::from_millisecond(ms) {
        Ok(ts) => ts
            .to_zoned_datetime(jiff::tz::TimeZone::system())
            .strftime("%Y-%m-%d %H:%M")
            .to_string(),
        Err(_) => "?".into(),
    }
}

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    let block = Block::new().title(" details ".bold());
    let inner = block.inner(area);
    f.render_widget(block, area);
    let Some(t) = app.selected().and_then(|id| app.tree.get(id)) else {
        return;
    };
    let now = app.repo.now_ms();
    let label = |s: &'static str| Span::styled(format!("{s:<9}"), Style::new().fg(Color::Gray));
    let mut lines = vec![
        Line::from(Span::styled(t.title.clone(), Style::new().bold())),
        Line::from(
            app.tree
                .path(t.id)
                .iter()
                .rev()
                .skip(1)
                .rev()
                .cloned()
                .collect::<Vec<_>>()
                .join(" › "),
        )
        .dim(),
        Line::raw(""),
        Line::from(vec![
            label("status"),
            Span::raw(if t.is_done() { "done" } else { "todo" }),
        ]),
        Line::from(vec![label("priority"), Span::raw(t.priority.to_string())]),
        score_line(app.scores.get(&t.id), label("score")),
        Line::from(vec![
            label("due"),
            match t.due {
                Some(d) => Span::styled(d.to_string(), due_style(d)),
                None => Span::raw("-"),
            },
        ]),
        Line::from(vec![
            label("tags"),
            Span::styled(
                if t.tags.is_empty() {
                    "-".into()
                } else {
                    t.tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" ")
                },
                Style::new().fg(Color::Cyan),
            ),
        ]),
        Line::from(vec![
            label("author"),
            Span::raw(t.author.as_ref().map_or_else(|| "-".to_owned(), |a| a.to_string())),
        ]),
    ];
    let current_tab = app.tab().map(|t| t.id);
    let dep_line = |d: &DepInfo| {
        let (mark, st) = match d.state {
            DepState::Done => ("✓ ", Style::new().fg(Color::Green)),
            DepState::Todo => ("○ ", Style::new()),
            DepState::Deleted => ("✗ ", Style::new().dim()),
            DepState::Missing | DepState::UnknownTab => ("⚠ ", Style::new().fg(Color::Red)),
        };
        let title = match &d.title {
            Some(t) if t.is_empty() => "(untitled)".to_owned(),
            Some(t) => t.clone(),
            None => format!("unknown task {}", short_id(d.task)),
        };
        let mut spans = vec![
            Span::raw("         "),
            Span::styled(mark, st),
            Span::styled(
                title,
                if d.state == DepState::Todo {
                    Style::new()
                } else {
                    Style::new().dim()
                },
            ),
        ];
        if d.tab.is_some() && d.tab != current_tab {
            let tab = match (&d.tab_name, d.tab) {
                (Some(n), _) => n.clone(),
                (None, Some(id)) => format!("tab {}", short_id(id)),
                (None, None) => "?".into(),
            };
            spans.push(Span::raw(format!(" [{tab}]")).fg(Color::Blue));
        }
        let mut out = vec![Line::from(spans)];
        if let Some(p) = dep_problem(d.state) {
            out.push(Line::from(vec![Span::raw("           "), Span::styled(p, st)]));
        }
        out
    };
    for (name, deps) in [
        ("requires", &app.detail_deps.requires),
        ("blocks", &app.detail_deps.blocks),
    ] {
        let Some((first, rest)) = deps.split_first() else {
            continue;
        };
        let mut first = dep_line(first);
        first[0].spans[0] = label(name);
        if name == "requires" && !t.is_done() {
            let open = deps.iter().filter(|d| d.state == DepState::Todo).count();
            if open > 0 {
                first[0].spans.push(Span::styled(
                    format!("   ⊘ {open} to do first"),
                    Style::new().fg(Color::Red),
                ));
            }
        }
        lines.extend(first);
        lines.extend(rest.iter().flat_map(dep_line));
    }
    if app.next == Some(t.id) {
        lines.push(Line::from(Span::styled(
            "★ next task in this tab",
            Style::new().fg(Color::Green),
        )));
    }
    lines.push(Line::raw(""));
    if !t.notes.is_empty() {
        lines.push(Line::from("notes".underlined()));
        lines.extend(t.notes.lines().map(|l| Line::raw(l.to_owned())));
        lines.push(Line::raw(""));
    }
    let total: i64 = app.detail_sessions.iter().map(|s| s.duration_ms(now)).sum();
    lines.push(Line::from(vec![
        "work log".underlined(),
        Span::raw(format!("  total {}", fmt_duration(total))),
    ]));
    for s in app.detail_sessions.iter().take(50) {
        lines.push(session_line(s, now));
    }
    f.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), inner);
}

/// `score    750  priority 300 + due 50 + blocking 400`
fn score_line(score: Option<&Score>, label: Span<'static>) -> Line<'static> {
    let Some(s) = score else {
        return Line::from(vec![label, Span::raw("-").dim()]);
    };
    let mut parts = vec![];
    for (name, v) in [("priority", s.priority), ("due", s.due), ("blocking", s.blocking)] {
        if v != 0 {
            parts.push(format!("{name} {v}"));
        }
    }
    let mut spans = vec![label, Span::styled(s.total().to_string(), Style::new().bold())];
    if parts.len() > 1 || s.priority == 0 && !parts.is_empty() {
        spans.push(Span::raw(format!("  {}", parts.join(" + "))).dim());
    }
    Line::from(spans)
}

/// Why a dependency cannot be checked, if it cannot.
pub fn dep_problem(state: DepState) -> Option<&'static str> {
    match state {
        DepState::Todo | DepState::Done => None,
        DepState::Deleted => Some("deleted"),
        DepState::Missing => Some("not found (not synced yet?)"),
        DepState::UnknownTab => Some("in a tab you don't have"),
    }
}

fn session_line(s: &Session, now: i64) -> Line<'static> {
    let mut v = vec![
        Span::raw(s.start.map(fmt_time).unwrap_or_default()),
        Span::raw(format!(" {:>6} ", fmt_duration(s.duration_ms(now)))),
        Span::styled(s.user.clone(), Style::new().fg(Color::Magenta)),
    ];
    if s.is_open() {
        v.push(Span::styled(" running", Style::new().fg(Color::Green)));
    }
    if !s.note.is_empty() {
        v.push(Span::raw(format!("  {}", s.note)).italic());
    }
    Line::from(v)
}

fn fmt_clock(ms: i64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    if let Mode::Command(ed) = &app.mode {
        f.render_widget(Paragraph::new(format!(":{}", ed.text())), area);
        f.set_cursor_position(Position::new(area.x + 1 + ed.cursor_col() as u16, area.y));
        return;
    }
    if let Mode::Tags { editor: ed, .. } = &app.mode {
        const PROMPT: &str = "tags (space-separated): ";
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::raw(PROMPT).fg(Color::Cyan),
                Span::raw(ed.text()),
            ])),
            area,
        );
        f.set_cursor_position(Position::new(area.x + (PROMPT.len() + ed.cursor_col()) as u16, area.y));
        return;
    }
    let (mode, mode_style) = match app.mode {
        Mode::Insert { .. } => (" INSERT ", Style::new().bg(Color::Green).fg(Color::Black).bold()),
        Mode::Search(_) => (" FIND ", Style::new().bg(Color::Yellow).fg(Color::Black).bold()),
        _ => (" NORMAL ", Style::new().bg(Color::Blue).fg(Color::White).bold()),
    };
    let mut left = vec![Span::styled(mode, mode_style), Span::raw(" ")];
    if let Some(m) = &app.message {
        left.push(Span::styled(
            m.text.clone(),
            if m.error {
                Style::new().fg(Color::Red)
            } else {
                Style::new()
            },
        ));
    } else if !app.pending.is_empty() {
        left.push(Span::raw(app.pending.clone()).dim());
    }
    let mut right = vec![];
    if let Some(s) = &app.active {
        let title = s
            .task
            .and_then(|id| app.tree.get(id).map(|t| t.title.clone()))
            .or_else(|| s.task.and_then(|id| app.repo.task(id).ok().map(|t| t.title)))
            .unwrap_or_default();
        right.push(Span::styled(
            format!("⏱ {} {} ", title, fmt_clock(s.duration_ms(app.repo.now_ms()))),
            Style::new().fg(Color::Green).bold(),
        ));
    }
    if let Some(s) = &app.sync_status {
        right.push(Span::raw(format!("{s} ")).dim());
    }
    let right = Line::from(right);
    let rw = right.width() as u16;
    let [l, r] = Layout::horizontal([Constraint::Min(1), Constraint::Length(rw)]).areas(area);
    f.render_widget(Paragraph::new(Line::from(left)), l);
    f.render_widget(Paragraph::new(right), r);
}

/// The Ctrl-P finder, centered over `body`.
fn render_search(f: &mut Frame, search: &Search, body: Rect) {
    let width = body.width.saturating_sub(4).min(80);
    let rows = search.hits.len().clamp(1, 12) as u16;
    let height = (rows + 4).min(body.height);
    if width < 20 || height < 5 {
        return;
    }
    let area = Rect {
        x: body.x + (body.width - width) / 2,
        y: body.y + 1.min(body.height - height),
        width,
        height,
    };
    let block = Block::bordered()
        .title(search.title.clone().bold())
        .title_bottom(Line::from(" ↑/↓ select · Enter go · Esc cancel ").right_aligned().dim())
        .border_style(Style::new().fg(Color::Yellow));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);
    let [input, sep, list] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1), Constraint::Min(1)]).areas(inner);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::raw("> ").fg(Color::Yellow).bold(),
            Span::raw(search.query()),
        ])),
        input,
    );
    f.set_cursor_position(Position::new(input.x + 2 + search.editor.cursor_col() as u16, input.y));
    f.render_widget(
        Paragraph::new(format!("{} / {}", search.hits.len(), search.candidates.len())).dim(),
        sep,
    );
    if search.hits.is_empty() {
        f.render_widget(Paragraph::new("no match").dim(), list);
        return;
    }
    let items: Vec<ListItem> = search
        .hits
        .iter()
        .map(|h| {
            let c = &search.candidates[h.candidate];
            let base = if c.done {
                Style::new().dim().crossed_out()
            } else {
                Style::new()
            };
            let mut spans: Vec<Span> = vec![];
            if c.title.is_empty() {
                spans.push(Span::styled("(untitled)", base.dim()));
            }
            for (i, ch) in c.title.chars().enumerate() {
                let st = if h.positions.binary_search(&i).is_ok() {
                    base.fg(Color::Yellow).bold()
                } else {
                    base
                };
                spans.push(Span::styled(ch.to_string(), st));
            }
            let mut ctx = vec![];
            if Some(c.tab) != search.current_tab {
                ctx.push(format!("[{}]", c.tab_name));
            }
            if !c.context.is_empty() {
                ctx.push(c.context.clone());
            }
            if !ctx.is_empty() {
                spans.push(Span::raw(format!("  {}", ctx.join(" "))).dim());
            }
            ListItem::new(Line::from(spans))
        })
        .collect();
    let mut state = ListState::default().with_selected(Some(search.selected));
    f.render_stateful_widget(
        List::new(items)
            .highlight_symbol(Line::from(SELECTED_SYMBOL).fg(Color::Yellow).bold())
            .highlight_spacing(HighlightSpacing::Always)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        list,
        &mut state,
    );
}

/// Time spent per task over the selected period, in the lower-right corner of `body`.
fn render_time_box(f: &mut Frame, app: &App, body: Rect) {
    let (Some(tab), true) = (app.tab(), app.time_period != TimePeriod::Hidden) else {
        return;
    };
    let lines = timebox::compute(&app.repo, &app.tree, tab.id, app.time_period, &app.tz);
    let total: i64 = lines.iter().map(|l| l.ms).sum();
    let width = body.width.min(44);
    let max_rows = body.height.saturating_sub(4).max(1) as usize;
    let rows = lines.len().clamp(1, max_rows);
    let height = (rows as u16 + 2).min(body.height);
    if width < 16 || height < 3 {
        return;
    }
    let area = Rect {
        x: body.right() - width,
        y: body.bottom() - height,
        width,
        height,
    };
    let inner_w = width as usize - 2;
    let mut text: Vec<Line> = vec![];
    if lines.is_empty() {
        text.push(Line::from("nothing logged").dim());
    }
    let hidden = lines.len().saturating_sub(rows);
    for (i, l) in lines.iter().take(rows).enumerate() {
        if hidden > 0 && i == rows - 1 {
            let rest: i64 = lines[i..].iter().map(|l| l.ms).sum();
            text.push(time_row(
                &format!("… {} more", lines.len() - i),
                &timebox::fmt_human(rest),
                inner_w,
                Style::new().dim(),
            ));
            break;
        }
        let mut st = Style::new();
        if l.task.is_some() && l.task == app.active.as_ref().and_then(|s| s.task) {
            st = st.fg(Color::Green);
        }
        text.push(time_row(&l.title, &timebox::fmt_human(l.ms), inner_w, st));
    }
    let block = Block::bordered()
        .title(format!(" time · {} ", app.time_period.label()).bold())
        .title_bottom(Line::from(format!(" total {} ", timebox::fmt_human(total))).right_aligned())
        .border_style(Style::new().fg(Color::Cyan));
    f.render_widget(Clear, area);
    f.render_widget(Paragraph::new(text).block(block), area);
}

/// `title ……… duration`, the title truncated to fit `width`.
fn time_row(title: &str, dur: &str, width: usize, style: Style) -> Line<'static> {
    let dur_w = dur.width();
    let avail = width.saturating_sub(dur_w + 1);
    let mut t = String::new();
    let mut w = 0;
    for c in title.chars() {
        let cw = c.width().unwrap_or(0);
        if w + cw > avail {
            t.pop();
            t.push('…');
            w = t.width();
            break;
        }
        t.push(c);
        w += cw;
    }
    let pad = width.saturating_sub(w + dur_w);
    Line::from(vec![
        Span::styled(t, style),
        Span::raw(" ".repeat(pad)),
        Span::styled(dur.to_owned(), style.bold()),
    ])
}

fn render_help(f: &mut Frame, scroll: &mut u16, page: &mut u16) {
    let area = f.area();
    let lines = HELP.lines().count() as u16;
    let h = (lines + 2).min(area.height);
    let w = (HELP.lines().map(|l| l.width()).max().unwrap_or(0) as u16 + 4).min(area.width);
    let r = Rect {
        x: area.x + (area.width - w) / 2,
        y: area.y + (area.height - h) / 2,
        width: w,
        height: h,
    };
    *page = h.saturating_sub(2);
    *scroll = (*scroll).min(lines.saturating_sub(*page));
    let mut block = Block::bordered()
        .title(" help ".bold())
        .style(Style::new().add_modifier(Modifier::empty()));
    if lines > *page {
        let more = match (*scroll > 0, *scroll + *page < lines) {
            (true, true) => "↑↓",
            (false, _) => "↓",
            (true, false) => "↑",
        };
        block = block.title_bottom(Line::from(format!(" {more} j/k ^d/^u scroll · q close ")).right_aligned());
    }
    f.render_widget(Clear, r);
    f.render_widget(Paragraph::new(HELP).scroll((*scroll, 0)).block(block), r);
}
