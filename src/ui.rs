//! TUI rendering: a workspace picker screen, then the run view (header, epic
//! list, log, status footer).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, EpicStatus, EpicView, Phase};
use crate::workspace::Workspace;

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

/// Workspace picker screen shown before a run starts.
pub fn render_picker(f: &mut Frame, workspaces: &[Workspace], selected: usize) {
    let area = f.area();
    let items: Vec<ListItem> = workspaces
        .iter()
        .enumerate()
        .map(|(index, workspace)| {
            let marker = if index == selected { "> " } else { "  " };
            let style = if index == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!("{marker}{}  {}", workspace.name, workspace.path.display()),
                style,
            )]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Select workspace (up/down, Enter, q to quit) "),
    );
    f.render_widget(list, area);
}

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),
            Constraint::Length(app.epics.len().min(8) as u16 + 2),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(f, app, chunks[0]);
    render_epics(f, app, chunks[1]);
    render_log(f, app, chunks[2]);
    render_footer(f, app, chunks[3]);
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let info = vec![
        Line::from(vec![
            Span::styled("Goal      ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate(&app.goal, 70)),
        ]),
        Line::from(vec![
            Span::styled("Workspace ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                truncate(&app.workspace, 70),
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ];
    let info_p = Paragraph::new(info).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Agentic Orchestrator "),
    );
    f.render_widget(info_p, cols[0]);

    let ratio = if app.budget > 0.0 {
        (app.total_cost / app.budget).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let gauge = Gauge::default()
        .block(Block::default().borders(Borders::ALL).title(" Budget "))
        .gauge_style(Style::default().fg(if ratio > 0.85 {
            Color::Red
        } else {
            Color::Green
        }))
        .ratio(ratio)
        .label(format!("${:.3} / ${:.2}", app.total_cost, app.budget));
    f.render_widget(gauge, cols[1]);
}

fn status_glyph(status: EpicStatus) -> (&'static str, Color) {
    match status {
        EpicStatus::Running => ("running  ", Color::Yellow),
        EpicStatus::Verifying => ("verifying", Color::Yellow),
        EpicStatus::Merged => ("merged   ", Color::Green),
        EpicStatus::Failed => ("failed   ", Color::Red),
        EpicStatus::Skipped => ("skipped  ", Color::DarkGray),
        EpicStatus::Conflict => ("conflict ", Color::Magenta),
    }
}

fn render_epics(f: &mut Frame, app: &App, area: Rect) {
    let items: Vec<ListItem> = app
        .epics
        .iter()
        .map(|epic: &EpicView| {
            let (label, color) = status_glyph(epic.status);
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {label} "), Style::default().fg(color)),
                Span::raw(format!("{}  {}", epic.id, truncate(&epic.title, 60))),
            ]))
        })
        .collect();
    let list = List::new(items).block(Block::default().borders(Borders::ALL).title(" Epics "));
    f.render_widget(list, area);
}

fn render_log(f: &mut Frame, app: &App, area: Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let total = app.log.len();
    let start = total.saturating_sub(inner_h);
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .map(|l| Line::from(l.clone()))
        .collect();
    let paragraph = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Log "))
        .wrap(Wrap { trim: false });
    f.render_widget(paragraph, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let (icon, label, color) = match app.phase {
        Phase::Planning => (
            SPINNER[app.spinner % SPINNER.len()],
            format!("planning... {}s", app.elapsed_secs),
            Color::Yellow,
        ),
        Phase::Implementing => (
            SPINNER[app.spinner % SPINNER.len()],
            format!("implementing... {}s", app.elapsed_secs),
            Color::Yellow,
        ),
        Phase::Done => ("ok", "done".to_string(), Color::Green),
        Phase::Failed => ("x", "failed".to_string(), Color::Red),
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {icon} "),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(label, Style::default().fg(color)),
        Span::styled("   q: quit/abort", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}...")
    }
}
