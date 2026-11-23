//! Breakdown of where time was spent between two dates.

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use cpasfait_core::dates::{parse_date, parse_period, today_local};
use cpasfait_core::report::{self, Filter, fmt_duration};
use cpasfait_tools::open_db;
use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, Copy, PartialEq, Eq, ValueEnum)]
enum By {
    /// Per tab, per task, with subtasks rolled up into their parents.
    Task,
    Tab,
    /// Per combination of tags (a task has its ancestors' tags too).
    Tags,
    /// Per tag (a task with two tags counts for both: the total is larger).
    Tag,
    User,
    Day,
}

/// Where time was spent, in all tabs, during a period (local time zone).
///
/// Examples: `cpasfait-report last-week --by tab,tags --me`,
/// `cpasfait-report --from 2026-11-01 --to today`.
///
/// Dates accept YYYY-MM-DD, today, yesterday, +Nd/-Nd/-Nw/-Nm, or weekday names.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// today, yesterday, this-week (Monday to Sunday), last-week, this-month,
    /// last-month, this-year, last-year. Default: the last 7 days.
    #[arg(conflicts_with_all = ["from", "to"])]
    period: Option<String>,
    /// First day (default: 6 days ago).
    #[arg(short, long, allow_hyphen_values = true)]
    from: Option<String>,
    /// Last day, inclusive (default: today).
    #[arg(short, long, allow_hyphen_values = true)]
    to: Option<String>,
    /// Only this tab (name, unique prefix or id).
    #[arg(long)]
    tab: Option<String>,
    /// Only this user.
    #[arg(short, long, conflicts_with = "me")]
    user: Option<String>,
    /// Only my time (shared tabs also hold the other members' sessions).
    #[arg(long)]
    me: bool,
    /// Groupings, comma-separated or repeated: one section each.
    #[arg(short, long, value_enum, value_delimiter = ',', default_value = "task")]
    by: Vec<By>,
    #[arg(long)]
    db: Option<PathBuf>,
    /// JSON output.
    #[arg(long)]
    json: bool,
}

fn day_start_ms(d: Date, tz: &TimeZone) -> Result<i64> {
    Ok(d.to_zoned_datetime(tz.clone())?.timestamp().as_millisecond())
}

/// A "label  duration  percent" table.
fn table(rows: Vec<(String, i64)>, total: i64) -> String {
    let width = rows
        .iter()
        .map(|(l, _)| l.chars().count())
        .max()
        .unwrap_or(4)
        .clamp(4, 60);
    rows.into_iter()
        .map(|(label, ms)| format!("{label:<width$}  {:>7}  {}\n", fmt_duration(ms), pct(ms, total)))
        .collect()
}

fn pct(part: i64, total: i64) -> String {
    if total == 0 {
        String::new()
    } else {
        format!("{:5.1}%", part as f64 * 100.0 / total as f64)
    }
}

fn main() -> Result<()> {
    let mut out = String::new();
    let res = run(&mut out);
    cpasfait_tools::print_stdout(&out);
    res
}

fn run(out: &mut String) -> Result<()> {
    use std::fmt::Write;
    macro_rules! println {
        () => { out.push('\n') };
        ($($t:tt)*) => { writeln!(out, $($t)*).unwrap() };
    }
    let args = Args::parse();
    let today = today_local();
    let parse = |s: &str| parse_date(s, today).with_context(|| format!("can't parse date {s:?}"));
    let (from, to) = match &args.period {
        Some(p) => parse_period(p, today).with_context(|| {
            format!("unknown period {p:?} (today, yesterday, this-week, last-week, this-month, last-month, this-year, last-year)")
        })?,
        None => (
            match &args.from {
                Some(s) => parse(s)?,
                None => today.checked_sub(6.days())?,
            },
            match &args.to {
                Some(s) => parse(s)?,
                None => today,
            },
        ),
    };
    if to < from {
        bail!("--to ({to}) is before --from ({from})");
    }
    let tz = TimeZone::system();
    let (from_ms, to_ms) = (day_start_ms(from, &tz)?, day_start_ms(to.tomorrow()?, &tz)?);

    let repo = open_db(args.db)?;
    let tab = args.tab.as_deref().map(|q| repo.find_tab(q)).transpose()?;
    let filter = Filter {
        tab: tab.as_ref().map(|t| t.id),
        user: if args.me {
            Some(repo.user().to_owned())
        } else {
            args.user.clone()
        },
    };
    let slices = report::slices(&repo, from_ms, to_ms, &filter)?;
    let total: i64 = slices.iter().map(|s| s.ms()).sum();
    let days = (to - from).get_days() + 1;

    let mut groupings = args.by.clone();
    groupings.dedup();
    if args.json {
        let mut v = json!({"from": from.to_string(), "to": to.to_string(), "total_ms": total});
        for by in &groupings {
            let (key, value) = match by {
                By::Task => ("tasks", serde_json::to_value(report::by_task(&repo, &slices)?)?),
                By::Tab => (
                    "tabs",
                    report::by_task(&repo, &slices)?
                        .iter()
                        .map(|t| json!({"tab": t.tab, "name": t.name, "ms": t.total_ms}))
                        .collect(),
                ),
                By::Tags => (
                    "tag_sets",
                    report::by_tag_set(&repo, &slices)?
                        .iter()
                        .map(|(tags, ms)| json!({"tags": tags, "ms": ms}))
                        .collect(),
                ),
                By::Tag => (
                    "tags",
                    report::by_tag(&repo, &slices)?
                        .iter()
                        .map(|(tag, ms)| json!({"tag": tag, "ms": ms}))
                        .collect(),
                ),
                By::User => (
                    "users",
                    report::by_user(&slices)
                        .iter()
                        .map(|(u, ms)| json!({"user": u, "ms": ms}))
                        .collect(),
                ),
                By::Day => (
                    "days",
                    report::by_day(&slices, &tz)
                        .iter()
                        .map(|(d, ms)| json!({"day": d.to_string(), "ms": ms}))
                        .collect(),
                ),
            };
            v[key] = value;
        }
        println!("{}", serde_json::to_string_pretty(&v)?);
        return Ok(());
    }

    let who = match &filter.user {
        Some(u) => format!("   ({u})"),
        None => String::new(),
    };
    println!(
        "{from} → {to} ({days} day{})   total {}{who}",
        if days > 1 { "s" } else { "" },
        fmt_duration(total)
    );
    let table = |rows: Vec<(String, i64)>| table(rows, total);
    for by in &groupings {
        println!();
        if groupings.len() > 1 {
            let title = match by {
                By::Task => "By task",
                By::Tab => "By tab",
                By::Tags => "By tag combination",
                By::Tag => "By tag (a task counts for each of its tags)",
                By::User => "By user",
                By::Day => "By day",
            };
            println!("{title}");
        }
        match by {
            By::Task => {
                let tabs = report::by_task(&repo, &slices)?;
                let width = tabs
                    .iter()
                    .flat_map(|t| {
                        std::iter::once(t.name.chars().count())
                            .chain(t.lines.iter().map(|l| 2 + 2 * l.depth + l.title.chars().count()))
                    })
                    .max()
                    .unwrap_or(10)
                    .clamp(10, 60);
                for (i, t) in tabs.iter().enumerate() {
                    if i > 0 {
                        println!();
                    }
                    println!(
                        "{:<width$}  {:>7}  {}",
                        t.name,
                        fmt_duration(t.total_ms),
                        pct(t.total_ms, total)
                    );
                    for l in &t.lines {
                        let mut label = format!("{}{}", "  ".repeat(l.depth + 1), l.title);
                        if label.chars().count() > width {
                            label = label.chars().take(width - 1).collect::<String>() + "…";
                        }
                        let own = if l.own_ms > 0 && l.own_ms != l.total_ms {
                            format!("  (own {})", fmt_duration(l.own_ms))
                        } else {
                            String::new()
                        };
                        println!(
                            "{label:<width$}  {:>7}  {}{own}",
                            fmt_duration(l.total_ms),
                            pct(l.total_ms, total)
                        );
                    }
                }
            }
            By::Tab => {
                let mut rows: Vec<_> = report::by_task(&repo, &slices)?
                    .into_iter()
                    .map(|t| (t.name, t.total_ms))
                    .collect();
                rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
                out.push_str(&table(rows));
            }
            By::Tags => out.push_str(&table(
                report::by_tag_set(&repo, &slices)?
                    .into_iter()
                    .map(|(tags, ms)| {
                        let label = if tags.is_empty() {
                            "(no tags)".to_owned()
                        } else {
                            tags.iter().map(|t| format!("#{t}")).collect::<Vec<_>>().join(" + ")
                        };
                        (label, ms)
                    })
                    .collect(),
            )),
            By::Tag => out.push_str(&table(
                report::by_tag(&repo, &slices)?
                    .into_iter()
                    .map(|(tag, ms)| (tag.map_or_else(|| "(no tags)".to_owned(), |t| format!("#{t}")), ms))
                    .collect(),
            )),
            By::User => out.push_str(&table(report::by_user(&slices))),
            By::Day => out.push_str(&table(
                report::by_day(&slices, &tz)
                    .into_iter()
                    .map(|(d, ms)| (format!("{d} {}", d.strftime("%a")), ms))
                    .collect(),
            )),
        }
    }
    Ok(())
}
