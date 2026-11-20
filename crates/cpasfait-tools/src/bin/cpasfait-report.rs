//! Breakdown of where time was spent between two dates.

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use cpasfait_core::dates::{parse_date, today_local};
use cpasfait_core::report::{self, Filter, fmt_duration};
use cpasfait_tools::open_db;
use jiff::ToSpan;
use jiff::civil::Date;
use jiff::tz::TimeZone;
use serde_json::json;
use std::path::PathBuf;

#[derive(Clone, Copy, ValueEnum)]
enum By {
    /// Per tab, per task, with subtasks rolled up into their parents.
    Task,
    Tab,
    User,
    Day,
}

/// Where time was spent between two dates (inclusive, local time zone).
///
/// Dates accept YYYY-MM-DD, today, yesterday, +Nd/-Nd/-Nw/-Nm, or weekday names.
#[derive(Parser)]
#[command(version)]
struct Args {
    /// First day (default: 6 days ago, i.e. the last 7 days).
    #[arg(short, long, allow_hyphen_values = true)]
    from: Option<String>,
    /// Last day, inclusive (default: today).
    #[arg(short, long, allow_hyphen_values = true)]
    to: Option<String>,
    /// Only this tab (name, unique prefix or id).
    #[arg(long)]
    tab: Option<String>,
    /// Only this user.
    #[arg(short, long)]
    user: Option<String>,
    /// Grouping.
    #[arg(short, long, value_enum, default_value_t = By::Task)]
    by: By,
    #[arg(long)]
    db: Option<PathBuf>,
    /// JSON output.
    #[arg(long)]
    json: bool,
}

fn day_start_ms(d: Date, tz: &TimeZone) -> Result<i64> {
    Ok(d.to_zoned_datetime(tz.clone())?.timestamp().as_millisecond())
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
    let from = match &args.from {
        Some(s) => parse(s)?,
        None => today.checked_sub(6.days())?,
    };
    let to = match &args.to {
        Some(s) => parse(s)?,
        None => today,
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
        user: args.user.clone(),
    };
    let slices = report::slices(&repo, from_ms, to_ms, &filter)?;
    let total: i64 = slices.iter().map(|s| s.ms()).sum();
    let days = (to - from).get_days() + 1;

    if !args.json {
        println!(
            "{from} → {to} ({days} day{})   total {}",
            if days > 1 { "s" } else { "" },
            fmt_duration(total)
        );
        println!();
    }
    match args.by {
        By::Task | By::Tab => {
            let tabs = report::by_task(&repo, &slices)?;
            if args.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(
                        &json!({"from": from.to_string(), "to": to.to_string(), "total_ms": total, "tabs": tabs})
                    )?
                );
                return Ok(());
            }
            let width = tabs
                .iter()
                .flat_map(|t| {
                    std::iter::once(t.name.chars().count())
                        .chain(t.lines.iter().map(|l| 2 + 2 * l.depth + l.title.chars().count()))
                })
                .max()
                .unwrap_or(10)
                .clamp(10, 60);
            for t in &tabs {
                println!(
                    "{:<width$}  {:>7}  {}",
                    t.name,
                    fmt_duration(t.total_ms),
                    pct(t.total_ms, total)
                );
                if matches!(args.by, By::Task) {
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
                    println!();
                }
            }
        }
        By::User => {
            let users = report::by_user(&slices);
            if args.json {
                let v: Vec<_> = users.iter().map(|(u, ms)| json!({"user": u, "ms": ms})).collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            let width = users.iter().map(|(u, _)| u.chars().count()).max().unwrap_or(4).max(4);
            for (u, ms) in users {
                println!("{u:<width$}  {:>7}  {}", fmt_duration(ms), pct(ms, total));
            }
        }
        By::Day => {
            let by_day = report::by_day(&slices, &tz);
            if args.json {
                let v: Vec<_> = by_day
                    .iter()
                    .map(|(d, ms)| json!({"day": d.to_string(), "ms": ms}))
                    .collect();
                println!("{}", serde_json::to_string_pretty(&v)?);
                return Ok(());
            }
            for (d, ms) in by_day {
                println!("{d} {}  {:>7}", d.strftime("%a"), fmt_duration(ms));
            }
        }
    }
    Ok(())
}
