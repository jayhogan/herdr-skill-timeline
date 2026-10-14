//! Drawing a `View` with ratatui.

use chrono::{DateTime, Utc};
use ratatui::Frame;
use ratatui::style::{Modifier, Style};
use ratatui::text::Line;
use ratatui::widgets::Paragraph;

use crate::ago;
use crate::timeline::{Group, Row, Status, View};

const INDENT: &str = "   ";

/// Draws `view`, clamping `scroll` to the content height.
pub fn render(frame: &mut Frame, view: &View, now: DateTime<Utc>, scroll: &mut u16) {
    let area = frame.area();
    let lines = lines(view, now, area.width as usize);
    let max_scroll = lines.len().saturating_sub(area.height as usize);
    *scroll = (*scroll).min(u16::try_from(max_scroll).unwrap_or(u16::MAX));
    frame.render_widget(Paragraph::new(lines).scroll((*scroll, 0)), area);
}

/// Every line of the timeline at `width` columns.
pub fn lines(view: &View, now: DateTime<Utc>, width: usize) -> Vec<Line<'static>> {
    let mut out = vec![header(view, width)];
    for group in &view.groups {
        out.push(Line::from(heading(group, width)));
        out.extend(group_lines(group, now, width));
    }
    out
}

fn header(view: &View, width: usize) -> Line<'static> {
    let right = match view.status {
        Status::Ok => "⟳ 2s",
        Status::Unreachable => "⚠ herdr unreachable",
    };
    let left = if view.title.is_empty() {
        " Skill timeline".to_string()
    } else {
        format!(" Skill timeline · {}", view.title)
    };
    Line::from(spread(&left, right, width)).style(Style::new().add_modifier(Modifier::BOLD))
}

fn heading(group: &Group, width: usize) -> String {
    let short_id = group.pane_id.rsplit(':').next().unwrap_or(&group.pane_id);
    let mut text = format!("─ claude · {short_id}");
    if let Some(title) = &group.title {
        text.push_str(&format!(" \"{title}\""));
    }
    text.push(' ');
    let text = fit(&text, width);
    let fill = width.saturating_sub(text.chars().count());
    format!("{text}{}", "─".repeat(fill))
}

fn group_lines(group: &Group, now: DateTime<Utc>, width: usize) -> Vec<Line<'static>> {
    if group.rows.is_empty() {
        return vec![Line::from(fit(&format!("{INDENT}(no skills yet)"), width))];
    }
    let mut newest_done = false;
    group
        .rows
        .iter()
        .map(|row| match row {
            Row::NewSession => Line::from(fit(&format!("{INDENT}┄┄ new session ┄┄"), width)),
            Row::Skill { name, at } => {
                let line = Line::from(spread(
                    &format!("{INDENT}{name}"),
                    &ago::label(*at, now),
                    width,
                ));
                if newest_done {
                    line
                } else {
                    newest_done = true;
                    line.style(Style::new().add_modifier(Modifier::BOLD))
                }
            }
        })
        .collect()
}

/// `left` and `right` on one line of `width` columns: `right` flush right and
/// never truncated (unless it alone exceeds the width); `left` truncated
/// with `…` to make room.
fn spread(left: &str, right: &str, width: usize) -> String {
    let right_len = right.chars().count();
    if right_len + 1 > width {
        return fit(right, width);
    }
    let left = fit(left, width - right_len - 1);
    let gap = width - right_len - left.chars().count();
    format!("{left}{}{right}", " ".repeat(gap))
}

/// `s` cut to at most `max` characters, ending in `…` if cut.
fn fit(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    if max == 0 {
        return String::new();
    }
    let mut cut: String = s.chars().take(max - 1).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;
    use ratatui::buffer::Buffer;

    fn t(s: &str) -> DateTime<Utc> {
        DateTime::parse_from_rfc3339(s).unwrap().with_timezone(&Utc)
    }

    const NOW: &str = "2026-10-14T12:00:00Z";

    fn view() -> View {
        View {
            title: "core-8920".into(),
            status: Status::Ok,
            groups: vec![
                Group {
                    pane_id: "w1:p2".into(),
                    title: Some("store blockers".into()),
                    rows: vec![
                        Row::Skill {
                            name: "simplify".into(),
                            at: t("2026-10-14T11:57:00Z"),
                        },
                        Row::Skill {
                            name: "test-driven-development".into(),
                            at: t("2026-10-14T11:42:00Z"),
                        },
                        Row::NewSession,
                        Row::Skill {
                            name: "superpowers:brainstorming".into(),
                            at: t("2026-10-14T10:00:00Z"),
                        },
                    ],
                },
                Group {
                    pane_id: "w1:p4".into(),
                    title: None,
                    rows: vec![],
                },
            ],
        }
    }

    fn draw(view: &View, width: u16, height: u16, scroll: &mut u16) -> Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal.draw(|f| render(f, view, t(NOW), scroll)).unwrap();
        terminal.backend().buffer().clone()
    }

    fn text(buf: &Buffer) -> Vec<String> {
        let width = buf.area.width as usize;
        buf.content
            .chunks(width)
            .map(|row| row.iter().map(|c| c.symbol()).collect::<String>())
            .collect()
    }

    #[test]
    fn renders_groups_rows_and_dividers() {
        let buf = draw(&view(), 44, 9, &mut 0);
        assert_eq!(
            text(&buf),
            vec![
                " Skill timeline · core-8920             ⟳ 2s",
                "─ claude · p2 \"store blockers\" ─────────────",
                "   simplify                        3 min ago",
                "   test-driven-development        18 min ago",
                "   ┄┄ new session ┄┄                        ",
                "   superpowers:brainstorming        2 hr ago",
                "─ claude · p4 ──────────────────────────────",
                "   (no skills yet)                          ",
                "                                            ",
            ]
        );
    }

    #[test]
    fn newest_row_is_bold_and_others_are_not() {
        let buf = draw(&view(), 44, 9, &mut 0);
        assert!(buf[(3, 2)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(3, 3)].modifier.contains(Modifier::BOLD));
        assert!(!buf[(3, 5)].modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn narrow_pane_truncates_names_not_times() {
        let lines = text(&draw(&view(), 24, 9, &mut 0));
        assert_eq!(lines[3], "   test-driv… 18 min ago");
        assert_eq!(lines[5], "   superpowers… 2 hr ago");
    }

    #[test]
    fn tiny_pane_does_not_panic() {
        for width in 0..6 {
            draw(&view(), width, 3, &mut 0);
        }
    }

    #[test]
    fn unreachable_header() {
        let mut v = view();
        v.status = Status::Unreachable;
        let lines = text(&draw(&v, 44, 2, &mut 0));
        assert_eq!(lines[0], " Skill timeline · core-… ⚠ herdr unreachable");
    }

    #[test]
    fn scroll_is_clamped_to_content() {
        let mut scroll = 100;
        let buf = draw(&view(), 44, 4, &mut scroll);
        assert_eq!(scroll, 4); // 8 lines of content, 4 visible
        assert_eq!(
            text(&buf)[3],
            "   (no skills yet)                          "
        );
    }
}
