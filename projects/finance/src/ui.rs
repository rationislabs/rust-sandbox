//! The full-screen app: Overview, Transactions and Trend for one month at a time.

use anyhow::Result;
use finance::{
    CategoryLine, MonthSummary, Rules, Transaction, YearMonth, monthly_spending, months, summarize,
};
use ratatui::Frame;
use ratatui::crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Bar, BarChart, Block, Paragraph, Row, Table, Tabs};
use rust_decimal::Decimal;
use rust_decimal::prelude::ToPrimitive;

pub const MIN_SIZE: (u16, u16) = (70, 20);
const VIEWS: [&str; 3] = ["Overview", "Transactions", "Trend"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Overview,
    Transactions,
    Trend,
}

pub struct App {
    pub view: View,
    /// Index into `months`.
    pub month: usize,
    /// First visible row of the Transactions table.
    pub scroll: usize,
    months: Vec<YearMonth>,
    txns: Vec<Transaction>,
    rules: Rules,
}

impl App {
    /// `txns` must not be empty (`finance::load` refuses empty data).
    pub fn new(txns: Vec<Transaction>, rules: Rules) -> App {
        let months = months(&txns);
        App {
            view: View::Overview,
            month: months.len().saturating_sub(1),
            scroll: 0,
            months,
            txns,
            rules,
        }
    }

    fn current(&self) -> YearMonth {
        self.months[self.month]
    }

    pub fn month_label(&self) -> String {
        self.current().to_string()
    }

    pub fn month_rows(&self) -> Vec<&Transaction> {
        let month = self.current();
        self.txns
            .iter()
            .filter(|t| YearMonth::of(t.date) == month)
            .collect()
    }

    /// Returns `false` when the app should quit.
    pub fn on_key(&mut self, code: KeyCode) -> bool {
        match code {
            KeyCode::Char('q') | KeyCode::Esc => return false,
            KeyCode::Tab => {
                self.view = match self.view {
                    View::Overview => View::Transactions,
                    View::Transactions => View::Trend,
                    View::Trend => View::Overview,
                };
            }
            KeyCode::Left if self.month > 0 => {
                self.month -= 1;
                self.scroll = 0;
            }
            KeyCode::Right if self.month + 1 < self.months.len() => {
                self.month += 1;
                self.scroll = 0;
            }
            KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Down => {
                let last = self.month_rows().len().saturating_sub(1);
                self.scroll = (self.scroll + 1).min(last);
            }
            _ => {}
        }
        true
    }
}

/// Runs until `q` or `Esc`; `ratatui::run` restores the terminal on error and on panic.
pub fn run(mut app: App) -> Result<()> {
    ratatui::run(|terminal| {
        loop {
            terminal.draw(|f| draw(f, &app))?;
            if let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
                && !app.on_key(key.code)
            {
                return Ok(());
            }
        }
    })
}

pub fn draw(frame: &mut Frame, app: &App) {
    let area = frame.area();
    if area.width < MIN_SIZE.0 || area.height < MIN_SIZE.1 {
        let msg = format!(
            "Terminal too small: need at least {}×{}",
            MIN_SIZE.0, MIN_SIZE.1
        );
        frame.render_widget(Paragraph::new(msg), area);
        return;
    }
    let [title, body, help] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(area);
    let selected = match app.view {
        View::Overview => 0,
        View::Transactions => 1,
        View::Trend => 2,
    };
    let [tabs, month] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(12)]).areas(title);
    frame.render_widget(
        Tabs::new(VIEWS)
            .select(selected)
            .highlight_style(Style::new().add_modifier(Modifier::REVERSED)),
        tabs,
    );
    frame.render_widget(
        Line::from(format!("◀ {} ▶", app.month_label())).right_aligned(),
        month,
    );
    frame.render_widget(
        Line::from("Tab view · ←/→ month · ↑/↓ scroll · q quit").dark_gray(),
        help,
    );
    match app.view {
        View::Overview => overview(frame, app, body),
        View::Transactions => transactions(frame, app, body),
        View::Trend => trend(frame, app, body),
    }
}

fn money(d: Decimal) -> String {
    format!("{d:.2}")
}

fn overview(frame: &mut Frame, app: &App, area: Rect) {
    let s: MonthSummary = summarize(&app.txns, &app.rules, app.current());
    let rows = u16::try_from(s.categories.len()).unwrap_or(u16::MAX);
    let [totals, budgets, top] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(rows + 2),
        Constraint::Min(0),
    ])
    .areas(area);
    let net_style = if s.net.is_sign_negative() {
        Color::Red
    } else {
        Color::Green
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("Income   {:>10}", money(s.income))),
            Line::from(format!("Spending {:>10}", money(s.spending))),
            Line::from(vec![
                Span::raw("Net      "),
                Span::styled(format!("{:>10}", money(s.net)), net_style),
            ]),
        ]),
        totals,
    );
    let bar_width = usize::from(budgets.width.saturating_sub(2 + 14 + 22));
    let lines: Vec<Line> = s
        .categories
        .iter()
        .map(|c| budget_line(c, bar_width))
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Budget vs actual ")),
        budgets,
    );
    let lines: Vec<Line> = s
        .top
        .iter()
        .map(|t| {
            Line::from(format!(
                "{}  {:<30} {:>10}",
                t.date,
                t.payee,
                money(-t.amount)
            ))
        })
        .collect();
    frame.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(" Largest expenses ")),
        top,
    );
}

/// `Groceries     ██████░░░░  310.01 / 350.00`, red when over budget. No floats: the
/// filled width is computed in `Decimal`.
fn budget_line(c: &CategoryLine, width: usize) -> Line<'static> {
    let over = c.remaining().is_sign_negative() && !c.remaining().is_zero();
    let filled = if c.budget.is_zero() || over {
        width
    } else {
        (c.actual * Decimal::from(width) / c.budget)
            .trunc()
            .to_usize()
            .unwrap_or(width)
            .min(width)
    };
    let color = if over { Color::Red } else { Color::Green };
    Line::from(vec![
        Span::raw(format!("{:<14}", c.name)),
        Span::styled("█".repeat(filled), color),
        Span::styled("░".repeat(width - filled), Color::DarkGray),
        Span::styled(
            format!(" {:>9} / {:<9}", money(c.actual), money(c.budget)),
            if over {
                Style::new().red()
            } else {
                Style::new()
            },
        ),
    ])
}

fn transactions(frame: &mut Frame, app: &App, area: Rect) {
    let rows = app.month_rows().into_iter().skip(app.scroll).map(|t| {
        let category = match app.rules.categorize(&t.payee) {
            Some(c) => Span::raw(c.name.clone()),
            None => Span::styled("Uncategorized ?", Color::Yellow),
        };
        Row::new(vec![
            Line::from(t.date.to_string()),
            Line::from(t.payee.clone()),
            Line::from(category),
            Line::from(money(t.amount)).right_aligned(),
        ])
    });
    let table = Table::new(
        rows,
        [
            Constraint::Length(10),
            Constraint::Min(20),
            Constraint::Length(16),
            Constraint::Length(10),
        ],
    )
    .header(Row::new(["Date", "Payee", "Category", "Amount"]).bold())
    .block(Block::bordered().title(format!(" Transactions {} ", app.month_label())));
    frame.render_widget(table, area);
}

fn trend(frame: &mut Frame, app: &App, area: Rect) {
    let bars: Vec<Bar> = monthly_spending(&app.txns, &app.rules)
        .into_iter()
        .map(|(m, spent)| {
            // Bar height in whole euros (display only); the label carries the exact amount.
            Bar::new(spent.trunc().to_u64().unwrap_or(0))
                .label(m.to_string())
                .text_value(money(spent))
                .style(if m == app.current() {
                    Color::Cyan
                } else {
                    Color::Blue
                })
        })
        .collect();
    let chart = BarChart::new(bars)
        .bar_width(9)
        .bar_gap(2)
        .block(Block::bordered().title(" Spending per month "));
    frame.render_widget(chart, area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use finance::{DEMO_SEED, default_rules, generate};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        App::new(generate(DEMO_SEED), default_rules())
    }

    fn screen(app: &App, w: u16, h: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(w, h)).unwrap();
        terminal.draw(|f| draw(f, app)).unwrap();
        let buffer = terminal.backend().buffer();
        buffer
            .content()
            .chunks(usize::from(w))
            .map(|row| {
                row.iter()
                    .map(ratatui::buffer::Cell::symbol)
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn starts_on_last_month_overview() {
        let app = app();
        assert_eq!(app.view, View::Overview);
        assert_eq!(app.month_label(), "2026-10");
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn tab_cycles_views() {
        let mut app = app();
        for want in [View::Transactions, View::Trend, View::Overview] {
            assert!(app.on_key(KeyCode::Tab));
            assert_eq!(app.view, want);
        }
    }

    #[test]
    fn quit_keys() {
        assert!(!app().on_key(KeyCode::Char('q')));
        assert!(!app().on_key(KeyCode::Esc));
        assert!(app().on_key(KeyCode::Char('x')));
    }

    #[test]
    fn month_bounds() {
        let mut app = app();
        app.on_key(KeyCode::Right);
        assert_eq!(app.month_label(), "2026-10");
        for _ in 0..5 {
            app.on_key(KeyCode::Left);
        }
        assert_eq!(app.month_label(), "2026-05");
        app.on_key(KeyCode::Left);
        assert_eq!(app.month_label(), "2026-05");
    }

    #[test]
    fn scroll_clamps_and_resets() {
        let mut app = app();
        app.on_key(KeyCode::Tab);
        let rows = app.month_rows().len();
        for _ in 0..500 {
            app.on_key(KeyCode::Down);
        }
        assert_eq!(app.scroll, rows - 1);
        app.on_key(KeyCode::Left);
        assert_eq!(app.scroll, 0);
        app.on_key(KeyCode::Up);
        assert_eq!(app.scroll, 0);
    }

    #[test]
    fn renders_overview_80x24() {
        let text = screen(&app(), 80, 24);
        for want in ["Overview", "2026-10", "Groceries", "q quit"] {
            assert!(text.contains(want), "missing {want:?}:\n{text}");
        }
    }

    #[test]
    fn renders_other_views_without_panic() {
        let mut app = app();
        app.on_key(KeyCode::Tab);
        assert!(screen(&app, 80, 24).contains("Payee"));
        app.on_key(KeyCode::Tab);
        assert!(screen(&app, 80, 24).contains("2026-05"));
    }

    #[test]
    fn too_small_40x10() {
        assert!(screen(&app(), 40, 10).contains("Terminal too small"));
    }
}
