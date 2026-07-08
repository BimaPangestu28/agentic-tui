//! TUI rendering: header with goal + cost gauge, log panel, status footer.

use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, Paragraph, Wrap},
    Frame,
};

use crate::app::{App, Phase};

const SPINNER: [&str; 4] = ["|", "/", "-", "\\"];

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4), // header
            Constraint::Min(1),    // log
            Constraint::Length(1), // footer
        ])
        .split(area);

    render_header(f, app, chunks[0]);
    render_log(f, app, chunks[1]);
    render_footer(f, app, chunks[2]);
}

fn render_header(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(65), Constraint::Percentage(35)])
        .split(area);

    let info = vec![
        Line::from(vec![
            Span::styled("Goal   ", Style::default().fg(Color::DarkGray)),
            Span::raw(truncate(&app.goal, 80)),
        ]),
        Line::from(vec![
            Span::styled("Output ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                if app.out_path.is_empty() { "..." } else { &app.out_path },
                Style::default().fg(Color::Cyan),
            ),
        ]),
    ];
    let info_p = Paragraph::new(info)
        .block(Block::default().borders(Borders::ALL).title(" PRD Generator "));
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

fn render_log(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let inner_h = area.height.saturating_sub(2) as usize;
    let total = app.log.len();
    let start = total.saturating_sub(inner_h);
    let lines: Vec<Line> = app
        .log
        .iter()
        .skip(start)
        .map(|l| Line::from(l.clone()))
        .collect();
    let p = Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL).title(" Log "))
        .wrap(Wrap { trim: false });
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, app: &App, area: ratatui::layout::Rect) {
    let (icon, label, color) = match app.phase {
        Phase::Running => (
            SPINNER[app.spinner % SPINNER.len()],
            format!("generating... {}s", app.elapsed_secs),
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
        Span::styled("   q: quit", Style::default().fg(Color::DarkGray)),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{t}…")
    }
}
