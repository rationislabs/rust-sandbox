use crate::model::Task;
use anyhow::{Result, bail};
use chrono::NaiveDate;
use std::fmt::Write;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DueFilter {
    /// Due today or earlier.
    Today,
    /// Due before today and not done.
    Overdue,
    OnOrBefore(NaiveDate),
}

pub fn parse_due(s: &str) -> Result<DueFilter> {
    match s {
        "today" => Ok(DueFilter::Today),
        "overdue" => Ok(DueFilter::Overdue),
        _ => match NaiveDate::parse_from_str(s, "%Y-%m-%d") {
            Ok(date) => Ok(DueFilter::OnOrBefore(date)),
            Err(_) => bail!("--due takes today, overdue or a date YYYY-MM-DD, not {s:?}"),
        },
    }
}

pub fn is_overdue(task: &Task, today: NaiveDate) -> bool {
    !task.done && task.due.is_some_and(|d| d < today)
}

/// Overdue first, then by due date, undated after dated, ties by id.
pub fn select<'a>(
    tasks: &'a [Task],
    all: bool,
    due: Option<DueFilter>,
    tag: Option<&str>,
    today: NaiveDate,
) -> Vec<&'a Task> {
    let mut out: Vec<&Task> = tasks
        .iter()
        .filter(|t| all || !t.done)
        .filter(|t| match due {
            None => true,
            Some(DueFilter::Today) => t.due.is_some_and(|d| d <= today),
            Some(DueFilter::Overdue) => is_overdue(t, today),
            Some(DueFilter::OnOrBefore(limit)) => t.due.is_some_and(|d| d <= limit),
        })
        .filter(|t| tag.is_none_or(|tag| t.tags.iter().any(|x| x == tag)))
        .collect();
    out.sort_by_key(|t| (!is_overdue(t, today), t.due.is_none(), t.due, t.id));
    out
}

pub fn render(task: &Task, today: NaiveDate) -> String {
    let mark = if task.done { "[x]" } else { "[ ]" };
    let due = match task.due {
        Some(d) if is_overdue(task, today) => format!("overdue {d}"),
        Some(d) => d.to_string(),
        None => String::new(),
    };
    let mut line = format!("{:>3} {mark} {due:<18} {}", task.id, task.text);
    for tag in &task.tags {
        let _ = write!(line, " #{tag}");
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn d(y: i32, m: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, day).unwrap()
    }

    fn task(id: u32, due: Option<NaiveDate>, tags: &[&str], done: bool) -> Task {
        Task {
            id,
            text: format!("task {id}"),
            due,
            tags: tags.iter().map(|t| (*t).to_string()).collect(),
            done,
        }
    }

    fn fixture() -> Vec<Task> {
        vec![
            task(1, None, &[], false),
            task(2, Some(d(2026, 10, 20)), &[], false),
            task(3, Some(d(2026, 10, 28)), &["home"], false),
            task(4, Some(d(2026, 11, 5)), &[], false),
            task(5, Some(d(2026, 10, 1)), &[], true),
        ]
    }

    const TODAY: NaiveDate = NaiveDate::from_ymd_opt(2026, 10, 28).unwrap();

    fn ids(tasks: &[&Task]) -> Vec<u32> {
        tasks.iter().map(|t| t.id).collect()
    }

    #[test]
    fn default_order() {
        assert_eq!(
            ids(&select(&fixture(), false, None, None, TODAY)),
            [2, 3, 4, 1]
        );
    }

    #[test]
    fn all_includes_done_never_overdue() {
        let tasks = fixture();
        assert_eq!(
            ids(&select(&tasks, true, None, None, TODAY)),
            [2, 5, 3, 4, 1]
        );
        assert!(!is_overdue(&tasks[4], TODAY));
    }

    #[test]
    fn due_today_includes_overdue() {
        let tasks = fixture();
        let got = select(&tasks, false, Some(DueFilter::Today), None, TODAY);
        assert_eq!(ids(&got), [2, 3]);
    }

    #[test]
    fn due_overdue() {
        let tasks = fixture();
        let got = select(&tasks, false, Some(DueFilter::Overdue), None, TODAY);
        assert_eq!(ids(&got), [2]);
    }

    #[test]
    fn due_on_or_before_date() {
        let filter = DueFilter::OnOrBefore(d(2026, 11, 5));
        let tasks = fixture();
        let got = select(&tasks, false, Some(filter), None, TODAY);
        assert_eq!(ids(&got), [2, 3, 4]);
    }

    #[test]
    fn tag_filter() {
        assert_eq!(
            ids(&select(&fixture(), false, None, Some("home"), TODAY)),
            [3]
        );
    }

    #[test]
    fn parse_due_forms() {
        assert_eq!(parse_due("today").unwrap(), DueFilter::Today);
        assert_eq!(parse_due("overdue").unwrap(), DueFilter::Overdue);
        assert_eq!(
            parse_due("2026-11-05").unwrap(),
            DueFilter::OnOrBefore(d(2026, 11, 5))
        );
        assert!(parse_due("tomorrow").is_err());
        assert!(parse_due("2026-13-01").is_err());
    }

    #[test]
    fn render_marks_overdue() {
        let tasks = fixture();
        assert!(render(&tasks[1], TODAY).contains("overdue"));
        assert!(!render(&tasks[2], TODAY).contains("overdue"));
        assert!(render(&tasks[4], TODAY).contains("[x]"));
    }
}
