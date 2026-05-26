use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{App, BrowserEntry, ParamEditorMode, Phase, TestStatus, TreeItem};

pub fn render(f: &mut Frame, app: &App) {
    let area = f.area();

    let outer = Layout::vertical([
        Constraint::Length(1), // header bar
        Constraint::Fill(1),   // test list
        Constraint::Min(6),    // detail panel (at least 6 lines)
        Constraint::Length(1), // status bar
    ])
    .split(area);

    // Clamp detail panel to at most 40% of total height.
    let max_detail = (area.height as f32 * 0.4) as u16;
    let detail_height = outer[2].height.min(max_detail).max(6);
    let list_height = area.height.saturating_sub(1 + detail_height + 1);

    let body = Layout::vertical([
        Constraint::Length(list_height),
        Constraint::Length(detail_height),
    ])
    .split(outer[1].union(outer[2]));

    render_header(f, app, outer[0]);
    render_list(f, app, body[0]);
    render_detail(f, app, body[1]);
    render_statusbar(f, app, outer[3]);

    if app.show_browser {
        render_browser(f, app);
    }
    if app.show_params {
        render_param_editor(f, app);
    }
    // Debug overlay is rendered last so it floats above everything else.
    if app.show_debug {
        render_debug_overlay(f, app);
    }
}

// ─── Header ───────────────────────────────────────────────────────────────────

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let left = match &app.phase {
        Phase::Running { current } => {
            format!(" {} {}  {}/{}", app.spinner(), app.title, current + 1, app.tests.len())
        },
        Phase::Done { elapsed_ms } => format!(" ✓ {}  done  ({} ms)", app.title, elapsed_ms),
        Phase::Error(msg) => format!(" ✗ {}  error: {}", app.title, msg),
    };

    let right = "  [q]quit  [jk]nav  [Enter]expand  [du]scroll  [l]logs  [o]open  [e]env  [r]rerun  [?]debug  ";
    let pad = (area.width as usize).saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(left, Style::new().bold()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::new().fg(Color::DarkGray)),
    ]);

    let bg = match &app.phase {
        Phase::Error(_) => Style::new().bg(Color::Red).fg(Color::White),
        _ => Style::new().bg(Color::DarkGray).fg(Color::White),
    };

    f.render_widget(Paragraph::new(line).style(bg), area);
}

// ─── Test list ────────────────────────────────────────────────────────────────

fn render_list(f: &mut Frame, app: &App, area: Rect) {
    let show_folders = app.folders.len() > 1;

    let items: Vec<ListItem> = app
        .tree
        .iter()
        .enumerate()
        .map(|(tree_idx, item)| {
            let selected = tree_idx == app.cursor;
            let row_style = if selected {
                Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };

            match item {
                TreeItem::Folder(fi) => {
                    let folder = &app.folders[*fi];
                    let (passed, failed, other) = app.folder_stats(*fi);
                    let total = passed + failed + other;
                    let done = passed + failed;
                    let arrow = if folder.collapsed { "▶ " } else { "▼ " };
                    let stats = format!("  ({}/{})", done, total);
                    let folder_style = if selected {
                        row_style
                    } else {
                        Style::new().fg(Color::Cyan)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled(arrow, folder_style),
                        Span::styled(
                            folder.name.as_str(),
                            folder_style.add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(stats, Style::new().fg(Color::DarkGray)),
                    ]))
                },
                TreeItem::Test(ti) => {
                    let t = &app.tests[*ti];
                    let (icon, icon_style) = match &t.status {
                        TestStatus::Pending => ("○ ", Style::new().fg(Color::DarkGray)),
                        TestStatus::Running => ("⋯ ", Style::new().fg(Color::Yellow)),
                        TestStatus::Passed => ("✓ ", Style::new().fg(Color::Green)),
                        TestStatus::Failed => ("✗ ", Style::new().fg(Color::Red)),
                    };

                    let name_style = match &t.status {
                        TestStatus::Pending => Style::new().fg(Color::DarkGray),
                        TestStatus::Running => Style::new().fg(Color::Yellow),
                        TestStatus::Passed => Style::new(),
                        TestStatus::Failed => Style::new().fg(Color::Red),
                    };

                    let time =
                        t.response_time_ms().map(|ms| format!("  {}ms", ms)).unwrap_or_default();

                    let indent = if show_folders { "  " } else { "" };

                    ListItem::new(Line::from(vec![
                        Span::raw(indent),
                        Span::styled(icon, if selected { row_style } else { icon_style }),
                        Span::styled(&t.name, if selected { row_style } else { name_style }),
                        Span::styled(time, Style::new().fg(Color::DarkGray)),
                    ]))
                },
            }
        })
        .collect();

    let block = Block::bordered().title(" Tests ");
    let list = List::new(items).block(block);
    let mut state = ListState::default().with_selected(Some(app.cursor));
    f.render_stateful_widget(list, area, &mut state);
}

// ─── Detail panel ─────────────────────────────────────────────────────────────

fn render_detail(f: &mut Frame, app: &App, area: Rect) {
    match app.tree.get(app.cursor) {
        Some(TreeItem::Folder(fi)) => render_folder_detail(f, app, *fi, area),
        Some(TreeItem::Test(ti)) => render_test_detail(f, app, *ti, area),
        None => {
            f.render_widget(Block::bordered().title(" Detail "), area);
        },
    }
}

fn render_folder_detail(f: &mut Frame, app: &App, fi: usize, area: Rect) {
    let folder = &app.folders[fi];
    let (passed, failed, pending) = app.folder_stats(fi);

    let lines = vec![
        Line::from(vec![
            Span::raw("Passed:  "),
            Span::styled(passed.to_string(), Style::new().fg(Color::Green).bold()),
        ]),
        Line::from(vec![
            Span::raw("Failed:  "),
            Span::styled(failed.to_string(), Style::new().fg(Color::Red).bold()),
        ]),
        Line::from(vec![
            Span::raw("Pending: "),
            Span::styled(pending.to_string(), Style::new().fg(Color::DarkGray)),
        ]),
        Line::default(),
        Line::from(Span::styled("  [Enter] expand / collapse", Style::new().fg(Color::DarkGray))),
    ];

    let state = if folder.collapsed {
        "collapsed"
    } else {
        "expanded"
    };
    let title = format!(" {} ({}) ", folder.name, state);
    f.render_widget(
        Paragraph::new(lines).block(Block::bordered().title(title)).wrap(Wrap { trim: false }),
        area,
    );
}

fn render_test_detail(f: &mut Frame, app: &App, ti: usize, area: Rect) {
    let test = match app.tests.get(ti) {
        Some(t) => t,
        None => {
            f.render_widget(Block::bordered().title(" Detail "), area);
            return;
        },
    };

    let mut lines: Vec<Line> = Vec::new();

    match &test.status {
        TestStatus::Pending => {
            lines.push(Line::from(Span::styled("Not yet run.", Style::new().fg(Color::DarkGray))));
        },
        TestStatus::Running => {
            lines.push(Line::from(Span::styled("Running…", Style::new().fg(Color::Yellow))));
        },
        TestStatus::Passed | TestStatus::Failed => {
            if let Some(result) = &test.result {
                if let Some(resp) = &result.response {
                    let status_color = match resp.status {
                        200..=299 => Color::Green,
                        300..=399 => Color::Yellow,
                        _ => Color::Red,
                    };
                    lines.push(Line::from(vec![
                        Span::raw("HTTP "),
                        Span::styled(resp.status.to_string(), Style::new().fg(status_color).bold()),
                        Span::styled(
                            format!("    {} ms", resp.response_time_ms),
                            Style::new().fg(Color::DarkGray),
                        ),
                    ]));
                }

                if !result.failures.is_empty() {
                    lines.push(Line::default());
                    lines.push(Line::from(Span::styled(
                        "Failures:",
                        Style::new().fg(Color::Red).bold(),
                    )));
                    for msg in &result.failures {
                        lines.push(Line::from(Span::styled(
                            format!("  · {}", msg),
                            Style::new().fg(Color::Red),
                        )));
                    }
                }

                if !result.logs.is_empty() {
                    lines.push(Line::default());
                    if app.show_logs {
                        lines.push(Line::from(Span::styled(
                            "Logs:",
                            Style::new().fg(Color::Cyan).bold(),
                        )));
                        for entry in &result.logs {
                            for log_line in entry.lines() {
                                lines.push(Line::from(Span::styled(
                                    format!("  {}", log_line),
                                    Style::new().fg(Color::Cyan),
                                )));
                            }
                        }
                    } else {
                        lines.push(Line::from(Span::styled(
                            format!(
                                "  {} log line(s) hidden — press [l] to show",
                                result.logs.len()
                            ),
                            Style::new().fg(Color::DarkGray),
                        )));
                    }
                }
            }
        },
    }

    let title = format!(" {} ", test.name);
    let block = Block::bordered().title(title);
    f.render_widget(
        Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}

// ─── Status bar ───────────────────────────────────────────────────────────────

fn render_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let (passed, failed) = app.summary();

    let (text, style) = match &app.phase {
        Phase::Running { .. } => (
            format!("  {} passed  {} failed  (running…)", passed, failed),
            Style::new().bg(Color::DarkGray).fg(Color::White),
        ),
        Phase::Done { elapsed_ms } => {
            let s = format!("  {} passed  {} failed  ({} ms total)", passed, failed, elapsed_ms);
            let style = if failed > 0 {
                Style::new().bg(Color::Red).fg(Color::White).bold()
            } else {
                Style::new().bg(Color::Green).fg(Color::Black).bold()
            };
            (s, style)
        },
        Phase::Error(msg) => {
            (format!("  Error: {}", msg), Style::new().bg(Color::Red).fg(Color::White).bold())
        },
    };

    f.render_widget(Paragraph::new(text).style(style), area);
}

// ─── File browser overlay ─────────────────────────────────────────────────────

fn render_browser(f: &mut Frame, app: &App) {
    let area = centered_rect(65, 82, f.area());
    f.render_widget(Clear, area);

    let cwd_str = app.browser.cwd.display().to_string();
    let title = format!(" Open: {} ", clip(&cwd_str, area.width.saturating_sub(10) as usize));
    let block = Block::bordered().title(title).style(Style::new().bg(Color::Black));

    let inner = block.inner(area);
    f.render_widget(block, area);

    let content_h = inner.height.saturating_sub(1);
    let content_rect = Rect::new(inner.x, inner.y, inner.width, content_h);
    let footer_rect = Rect::new(inner.x, inner.y + content_h, inner.width, 1);

    let browser = &app.browser;
    let items: Vec<ListItem> = browser
        .entries
        .iter()
        .enumerate()
        .map(|(i, entry)| {
            let selected = i == browser.cursor;
            let sel_style = Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD);

            match entry {
                BrowserEntry::ParentDir => ListItem::new(Line::from(Span::styled(
                    "  ↑  ..",
                    if selected {
                        sel_style
                    } else {
                        Style::new().fg(Color::DarkGray)
                    },
                ))),
                BrowserEntry::Dir(name, _) => {
                    let style = if selected {
                        sel_style
                    } else {
                        Style::new().fg(Color::Cyan)
                    };
                    ListItem::new(Line::from(vec![
                        Span::styled("  ▶  ", style),
                        Span::styled(name.as_str(), style.add_modifier(Modifier::BOLD)),
                        Span::styled("/", Style::new().fg(Color::DarkGray)),
                    ]))
                },
                BrowserEntry::HttpFile(name, _) => ListItem::new(Line::from(vec![
                    Span::raw("     "),
                    Span::styled(name.as_str(), if selected { sel_style } else { Style::new() }),
                ])),
            }
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default().with_selected(Some(browser.cursor));
    f.render_stateful_widget(list, content_rect, &mut state);

    let footer = "  [jk/↑↓] nav  [Enter] enter dir / open file  [Space] open this dir  [h/←/BS] back  [Esc] close";
    f.render_widget(Paragraph::new(footer).style(Style::new().fg(Color::DarkGray)), footer_rect);
}

// ─── Param editor overlay ─────────────────────────────────────────────────────

fn render_param_editor(f: &mut Frame, app: &App) {
    let area = centered_rect(72, 72, f.area());
    f.render_widget(Clear, area);

    let block =
        Block::bordered().title(" Environment Params ").style(Style::new().bg(Color::Black));

    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner: scrollable content area above a 1-line footer.
    let content_h = inner.height.saturating_sub(1);
    let content_rect = Rect::new(inner.x, inner.y, inner.width, content_h);
    let footer_rect = Rect::new(inner.x, inner.y + content_h, inner.width, 1);

    let editor = &app.param_editor;
    let key_col = 28usize;
    let sep = " │ ";
    let val_col = (inner.width as usize).saturating_sub(key_col + sep.len());

    // Header row + blank separator.
    let mut lines: Vec<Line> = vec![
        Line::from(vec![
            Span::styled(
                format!("{:<key_col$}", "KEY"),
                Style::new().fg(Color::DarkGray).underlined(),
            ),
            Span::styled(sep, Style::new().fg(Color::DarkGray)),
            Span::styled("VALUE", Style::new().fg(Color::DarkGray).underlined()),
        ]),
        Line::default(),
    ];

    for (i, (key, val)) in editor.rows.iter().enumerate() {
        let selected = i == editor.cursor;

        let key_display = if selected && editor.mode == ParamEditorMode::EditKey {
            format!("{}▌", editor.input)
        } else {
            key.clone()
        };
        let val_display = if selected && editor.mode == ParamEditorMode::EditValue {
            format!("{}▌", editor.input)
        } else {
            val.clone()
        };

        let row_bg = if selected {
            Color::DarkGray
        } else {
            Color::Reset
        };
        let val_style = if selected && editor.mode == ParamEditorMode::EditValue {
            Style::new().bg(row_bg).fg(Color::Yellow)
        } else {
            Style::new().bg(row_bg)
        };

        lines.push(Line::from(vec![
            Span::styled(pad_clip(&key_display, key_col), Style::new().bg(row_bg)),
            Span::styled(sep, Style::new().fg(Color::DarkGray)),
            Span::styled(clip(&val_display, val_col), val_style),
        ]));
    }

    // Scroll to keep editor.cursor visible (header = 2 lines).
    let header_lines = 2usize;
    let visible = content_rect.height as usize;
    let row_offset = if editor.cursor + header_lines + 1 > visible {
        ((editor.cursor + header_lines + 1) - visible) as u16
    } else {
        0
    };

    f.render_widget(Paragraph::new(lines).scroll((row_offset, 0)), content_rect);

    let footer_text = if editor.is_editing() {
        "  [Enter] confirm  [Esc] cancel  [Backspace] delete char"
    } else {
        "  [jk] nav  [e/Enter] edit value  [Tab] edit key  [a] add  [d] del  [r] save & rerun  [Esc] close"
    };
    f.render_widget(
        Paragraph::new(footer_text).style(Style::new().fg(Color::DarkGray)),
        footer_rect,
    );
}

/// Pad with spaces or clip with `…` to exactly `width` chars.
fn pad_clip(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        format!("{s:<width$}")
    } else {
        let mut out: String = chars[..width.saturating_sub(1)].iter().collect();
        out.push('…');
        out
    }
}

/// Clip to at most `width` chars (no padding).
fn clip(s: &str, width: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= width {
        s.to_string()
    } else {
        let mut out: String = chars[..width.saturating_sub(1)].iter().collect();
        out.push('…');
        out
    }
}

// ─── Debug overlay ────────────────────────────────────────────────────────────

fn render_debug_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(82, 70, f.area());

    // Clear the background so the overlay has a clean slate.
    f.render_widget(Clear, area);

    let log_source = app.debug.entries();
    let total = log_source.len() as u16;

    // Last visible lines above the fixed footer.
    let inner_height = area.height.saturating_sub(3); // borders + footer
    let offset = app.debug_scroll.min(total.saturating_sub(inner_height));

    let mut lines: Vec<Line> = log_source
        .iter()
        .skip(offset as usize)
        .take(inner_height as usize)
        .map(|entry| {
            // Colour the timestamp prefix differently from the message body.
            if let Some(rest) = entry.strip_prefix('+')
                && let Some(space_pos) = rest.find("  ")
            {
                let ts = &rest[..space_pos];
                let msg = &rest[space_pos + 2..];
                return Line::from(vec![
                    Span::styled(format!("+{}  ", ts), Style::new().fg(Color::DarkGray)),
                    Span::raw(msg.to_string()),
                ]);
            }
            Line::from(entry.as_str())
        })
        .collect();

    // Pad to fill remaining lines so the footer stays at the bottom.
    while lines.len() < inner_height as usize {
        lines.push(Line::default());
    }

    // Footer: app state snapshot + scroll hint.
    let file_hint = if app.debug.has_file() {
        "  (mirrored to file)"
    } else {
        ""
    };
    let scroll_hint = format!(
        "  {}/{}{}  [[]scroll[]]  [?]close",
        offset + inner_height.min(total.saturating_sub(offset)),
        total,
        file_hint,
    );
    lines.push(Line::from(Span::styled(
        format!(
            "  {}{}",
            app.state_line(),
            " ".repeat(
                (area.width as usize)
                    .saturating_sub(2 + app.state_line().len() + scroll_hint.len())
            )
        ),
        Style::new().fg(Color::DarkGray),
    )));
    lines.push(Line::from(Span::styled(scroll_hint, Style::new().fg(Color::DarkGray))));

    let block = Block::bordered().title(" Debug Log ").style(Style::new().bg(Color::Black));

    f.render_widget(Paragraph::new(lines).block(block), area);
}

/// Return a [`Rect`] centred in `area` with the given percentage dimensions.
fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let w = (area.width * percent_x / 100).max(20);
    let h = (area.height * percent_y / 100).max(8);
    let x = area.x + area.width.saturating_sub(w) / 2;
    let y = area.y + area.height.saturating_sub(h) / 2;
    Rect::new(x, y, w.min(area.width), h.min(area.height))
}
