//! TUI rendering: a workspace picker screen, then the run view (header, epic
//! kanban board, log, status footer).

use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph, Wrap},
    Frame,
};

use crate::app::{is_on_hold, kanban_column, App, EpicStatus, EpicView, KanbanColumn, Phase};
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
            .title(" Select workspace (up/down, Enter, a to add, q to quit) "),
    );
    f.render_widget(list, area);
}

/// Goal input screen shown when no goal was given on the command line.
pub fn render_goal_input(f: &mut Frame, workspace: &str, buffer: &str) {
    let area = f.area();
    let title = format!(
        " Goal for {workspace} (Enter to run, end a line with \\ for a new line, Esc to cancel) "
    );
    let input = Paragraph::new(format!("{buffer}\u{2588}"))
        .wrap(Wrap { trim: false })
        .block(Block::default().borders(Borders::ALL).title(title));
    f.render_widget(input, area);
}

/// Status frame shown while a refine pass runs.
pub fn render_refining(f: &mut Frame, note: &str) {
    let area = f.area();
    let message = Paragraph::new(format!("Refining the goal: {note}..."))
        .wrap(Wrap { trim: true })
        .block(Block::default().borders(Borders::ALL).title(" Refine "));
    f.render_widget(message, area);
}

/// One clarifying question with an editable answer field.
pub fn render_refine_question(
    f: &mut Frame,
    question: &str,
    index: usize,
    total: usize,
    answer: &str,
) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(5),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);
    let prompt = Paragraph::new(question.to_string())
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" Question {index} of {total} ")),
        );
    f.render_widget(prompt, rows[0]);
    let input = Paragraph::new(Line::from(vec![
        Span::raw(answer.to_string()),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Answer (Enter next, empty to skip, Esc to skip refining) "),
    );
    f.render_widget(input, rows[1]);
}

/// The final refined goal in an editable field, confirmed before planning.
pub fn render_goal_confirm(f: &mut Frame, goal: &str) {
    let area = f.area();
    let input = Paragraph::new(format!("{goal}\u{2588}"))
        .wrap(Wrap { trim: false })
        .block(
            Block::default().borders(Borders::ALL).title(
                " Final goal (edit if needed, Enter to plan, end a line with \\ for a new line, Esc to use original) ",
            ),
        );
    f.render_widget(input, area);
}

/// Onboarding screen one: an editable path to scan for git repositories.
pub fn render_scan_root_input(f: &mut Frame, root: &str) {
    let area = f.area();
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);
    let input = Paragraph::new(Line::from(vec![
        Span::raw(root.to_string()),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]))
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Scan which folder for git repos? (Enter to scan, Esc to quit) "),
    );
    f.render_widget(input, rows[0]);
    let help = Paragraph::new(
        "No workspaces configured yet. Point at a folder and I will find the git repositories inside it.",
    )
    .wrap(Wrap { trim: true })
    .block(Block::default().borders(Borders::ALL).title(" Onboarding "));
    f.render_widget(help, rows[1]);
}

/// Onboarding screen two: a brief status while the scan runs.
pub fn render_scanning(f: &mut Frame, root: &str) {
    let area = f.area();
    let message = Paragraph::new(format!("Scanning {root} for git repositories..."))
        .block(Block::default().borders(Borders::ALL).title(" Onboarding "));
    f.render_widget(message, area);
}

/// Onboarding screen three: a checklist of found repositories.
pub fn render_repo_checklist(
    f: &mut Frame,
    repos: &[Workspace],
    selected: usize,
    checked: &[bool],
    root: &str,
) {
    let area = f.area();
    if repos.is_empty() {
        let message = Paragraph::new(format!(
            "No git repositories found under {root}.\nPress r to scan a different folder, or Esc to quit."
        ))
        .wrap(Wrap { trim: true })
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" No repositories found "),
        );
        f.render_widget(message, area);
        return;
    }
    let items: Vec<ListItem> = repos
        .iter()
        .enumerate()
        .map(|(index, workspace)| {
            let mark = if checked.get(index).copied().unwrap_or(false) {
                "[x]"
            } else {
                "[ ]"
            };
            let cursor = if index == selected { ">" } else { " " };
            let style = if index == selected {
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(vec![Span::styled(
                format!(
                    "{cursor} {mark} {}  {}",
                    workspace.name,
                    workspace.path.display()
                ),
                style,
            )]))
        })
        .collect();
    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Pick workspaces (up/down, Space toggle, Enter save, r rescan, Esc quit) "),
    );
    f.render_widget(list, area);
}

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),  // header
            Constraint::Min(8),     // board
            Constraint::Length(10), // log
            Constraint::Length(1),  // footer
        ])
        .split(area);

    render_header(f, app, chunks[0]);
    render_board(f, app, chunks[1]);
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

fn render_board(f: &mut Frame, app: &App, area: Rect) {
    use std::collections::HashMap;

    let status_by_id: HashMap<String, EpicStatus> = app
        .epics
        .iter()
        .map(|epic| (epic.id.clone(), epic.status))
        .collect();

    let columns = [
        (KanbanColumn::Todo, " Todo "),
        (KanbanColumn::InProgress, " In Progress "),
        (KanbanColumn::Review, " Review "),
        (KanbanColumn::Done, " Done "),
        (KanbanColumn::Blocked, " Blocked "),
    ];

    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(20); 5])
        .split(area);

    let card_width = cols[0].width.saturating_sub(2) as usize;
    let rows = area.height.saturating_sub(2) as usize;

    for (index, (column, title)) in columns.iter().enumerate() {
        let cards: Vec<&EpicView> = app
            .epics
            .iter()
            .filter(|epic| kanban_column(epic.status) == *column)
            .collect();

        // Reserve one row for the overflow line when needed.
        let visible = if cards.len() > rows {
            rows.saturating_sub(1)
        } else {
            cards.len()
        };

        let mut items: Vec<ListItem> = Vec::new();
        for epic in cards.iter().take(visible) {
            let (marker, color) = card_marker(epic, *column, &status_by_id);
            let label = if epic.title.is_empty() {
                epic.id.clone()
            } else {
                format!("{} {}", epic.id, epic.title)
            };
            let text = truncate(&format!("{marker} {label}"), card_width);
            items.push(ListItem::new(Line::from(Span::styled(
                text,
                Style::default().fg(color),
            ))));
        }
        if cards.len() > visible {
            items.push(ListItem::new(Line::from(Span::styled(
                format!("+{} more", cards.len() - visible),
                Style::default().fg(Color::DarkGray),
            ))));
        }

        let list = List::new(items).block(Block::default().borders(Borders::ALL).title(*title));
        f.render_widget(list, cols[index]);
    }
}

/// The short marker and color for an epic card, by column.
fn card_marker(
    epic: &EpicView,
    column: KanbanColumn,
    status_by_id: &std::collections::HashMap<String, EpicStatus>,
) -> (&'static str, Color) {
    match column {
        KanbanColumn::Todo => {
            if is_on_hold(&epic.depends_on, status_by_id) {
                ("hold", Color::DarkGray)
            } else {
                ("redy", Color::Cyan)
            }
        }
        KanbanColumn::InProgress => ("run ", Color::Yellow),
        KanbanColumn::Review => ("chk ", Color::Yellow),
        KanbanColumn::Done => ("ok  ", Color::Green),
        KanbanColumn::Blocked => match epic.status {
            EpicStatus::Failed => ("x   ", Color::Red),
            EpicStatus::Skipped => ("skip", Color::DarkGray),
            EpicStatus::Conflict => ("!   ", Color::Magenta),
            _ => ("    ", Color::Gray),
        },
    }
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
