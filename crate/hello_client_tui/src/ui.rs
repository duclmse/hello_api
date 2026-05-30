use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph, Wrap},
};

use crate::app::{
    App, BrowserEntry, CONVERT_FORMATS, DetailTab, ParamEditorMode, Phase, TestStatus, TreeItem,
    parse_multipart_fields,
};

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
    if app.show_convert {
        render_convert_overlay(f, app);
    }
    if app.show_request_editor {
        render_request_editor(f, app);
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
        Phase::Empty => " hello_tui  — press [o] to open a collection".to_string(),
        Phase::Idle => format!(" ○ {}  ready  ({} requests)", app.title, app.tests.len()),
        Phase::Running { current } => {
            format!(" {} {}  {}/{}", app.spinner(), app.title, current + 1, app.tests.len())
        },
        Phase::Done { elapsed_ms } => format!(" ✓ {}  done  ({} ms)", app.title, elapsed_ms),
        Phase::Error(msg) => format!(" ✗ {}  error: {}", app.title, msg),
    };

    let right = "  [q]quit  [jk]nav  [Enter]run  [Tab]req/result  [v]edit req  [du]scroll  [l]logs  [o]open  [C]convert  [e]env  [r]rerun  [?]debug  ";
    let pad = (area.width as usize).saturating_sub(left.len() + right.len());

    let line = Line::from(vec![
        Span::styled(left, Style::new().bold()),
        Span::raw(" ".repeat(pad)),
        Span::styled(right, Style::new().fg(Color::DarkGray)),
    ]);

    let bg = match &app.phase {
        Phase::Empty | Phase::Idle => Style::new().bg(Color::DarkGray).fg(Color::White),
        Phase::Error(_) => Style::new().bg(Color::Red).fg(Color::White),
        _ => Style::new().bg(Color::DarkGray).fg(Color::White),
    };

    f.render_widget(Paragraph::new(line).style(bg), area);
}

// ─── Test list ────────────────────────────────────────────────────────────────

fn render_list(f: &mut Frame, app: &App, area: Rect) {
    if matches!(app.phase, Phase::Empty) {
        let lines = vec![
            ratatui::text::Line::default(),
            ratatui::text::Line::from(Span::styled(
                "  No collection loaded.",
                Style::new().fg(Color::DarkGray),
            )),
            ratatui::text::Line::default(),
            ratatui::text::Line::from(Span::styled(
                "  [o]  open file browser to load a .http, .json, .bru, or .yaml collection",
                Style::new().fg(Color::DarkGray),
            )),
        ];
        f.render_widget(Paragraph::new(lines).block(Block::bordered().title(" Tests ")), area);
        return;
    }

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

    // Decide effective tab: force Request view when no result exists yet.
    let has_result = test.result.is_some();
    let effective_tab = if !has_result && app.detail_tab == DetailTab::Result {
        &DetailTab::Request
    } else {
        &app.detail_tab
    };

    // Build tab header spans.
    let req_style = if effective_tab == &DetailTab::Request {
        Style::new().bold().fg(Color::White)
    } else {
        Style::new().fg(Color::DarkGray)
    };
    let res_style = if effective_tab == &DetailTab::Result {
        Style::new().bold().fg(Color::White)
    } else {
        Style::new().fg(Color::DarkGray)
    };

    let title_line = Line::from(vec![
        Span::raw(" "),
        Span::styled(&test.name, Style::new().bold()),
        Span::styled("   [Tab] ", Style::new().fg(Color::DarkGray)),
        Span::styled("Request", req_style),
        Span::styled(" | ", Style::new().fg(Color::DarkGray)),
        Span::styled("Result", res_style),
        Span::raw(" "),
    ]);

    let block = Block::bordered().title(title_line);
    let inner = block.inner(area);
    f.render_widget(block, area);

    match effective_tab {
        DetailTab::Request => render_request_view(f, app, ti, inner),
        DetailTab::Result => render_result_view(f, app, test, inner),
    }
}

fn render_request_view(f: &mut Frame, app: &App, ti: usize, area: Rect) {
    let case = match app.cases.get(ti) {
        Some(c) => c,
        None => return,
    };

    let mut lines: Vec<Line> = Vec::new();

    lines.push(Line::from(vec![
        Span::styled("Method:  ", Style::new().fg(Color::DarkGray)),
        Span::styled(case.request.method.as_str(), Style::new().fg(Color::Cyan).bold()),
    ]));
    lines.push(Line::from(vec![
        Span::styled("URL:     ", Style::new().fg(Color::DarkGray)),
        Span::styled(case.request.url.as_str(), Style::new()),
    ]));

    if !case.request.headers.is_empty() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled("Headers:", Style::new().fg(Color::DarkGray))));
        for (k, v) in &case.request.headers {
            lines.push(Line::from(vec![
                Span::styled(format!("  {}: ", k), Style::new().fg(Color::DarkGray)),
                Span::raw(v.as_str()),
            ]));
        }
    }

    if let Some(body) = &case.request.body {
        if let Some(fields) = parse_multipart_fields(&case.request.headers, body) {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("Form Data:", Style::new().fg(Color::DarkGray))));
            if fields.is_empty() {
                lines.push(Line::from(Span::styled(
                    "  (no fields)",
                    Style::new().fg(Color::DarkGray),
                )));
            } else {
                for (name, value) in &fields {
                    lines.push(Line::from(vec![
                        Span::styled(format!("  {}:  ", name), Style::new().fg(Color::DarkGray)),
                        Span::styled(
                            value.lines().next().unwrap_or("").to_string(),
                            Style::new().fg(Color::Yellow),
                        ),
                    ]));
                }
            }
        } else {
            lines.push(Line::default());
            lines.push(Line::from(Span::styled("Body:", Style::new().fg(Color::DarkGray))));
            for body_line in body.lines() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", body_line),
                    Style::new().fg(Color::Yellow),
                )));
            }
        }
    }

    if case.pre_script.is_some() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Pre-script: (defined)",
            Style::new().fg(Color::DarkGray),
        )));
    }
    if case.post_script.is_some() {
        lines.push(Line::default());
        lines.push(Line::from(Span::styled(
            "Post-script: (defined)",
            Style::new().fg(Color::DarkGray),
        )));
    }

    lines.push(Line::default());
    lines.push(Line::from(Span::styled(
        "  [v] open editor  [Enter] run",
        Style::new().fg(Color::DarkGray),
    )));

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.detail_scroll, 0)),
        area,
    );
}

fn render_result_view(f: &mut Frame, app: &App, test: &crate::app::TestRow, area: Rect) {
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

    f.render_widget(
        Paragraph::new(lines).wrap(Wrap { trim: false }).scroll((app.detail_scroll, 0)),
        area,
    );
}

// ─── Request editor overlay ───────────────────────────────────────────────────

/// Split `template` into styled spans: literal text in `base_style`, `{{var}}` in
/// cyan (known) or dim-red (unknown), background always `bg`.
fn template_spans(
    template: &str,
    params: &std::collections::HashMap<String, String>,
    base_style: Style,
    bg: Color,
) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut rest = template;
    while let Some(open) = rest.find("{{") {
        if open > 0 {
            spans.push(Span::styled(rest[..open].to_string(), base_style.bg(bg)));
        }
        rest = &rest[open + 2..];
        if let Some(close) = rest.find("}}") {
            let var_name = &rest[..close];
            let var_style = if params.contains_key(var_name) {
                Style::new().fg(Color::Cyan).bg(bg).bold()
            } else {
                Style::new().fg(Color::Red).bg(bg).add_modifier(Modifier::DIM)
            };
            spans.push(Span::styled(format!("{{{{{}}}}}", var_name), var_style));
            rest = &rest[close + 2..];
        } else {
            spans.push(Span::styled(format!("{{{}{}", "{", rest), base_style.bg(bg)));
            rest = "";
        }
    }
    if !rest.is_empty() {
        spans.push(Span::styled(rest.to_string(), base_style.bg(bg)));
    }
    spans
}

/// Substitute `{{key}}` with values from `params`. Returns `Some(result)` only
/// if at least one substitution was made; `None` when the template has no vars.
fn resolve_vars(
    template: &str,
    params: &std::collections::HashMap<String, String>,
) -> Option<String> {
    if !template.contains("{{") {
        return None;
    }
    let mut result = template.to_string();
    for (k, v) in params {
        result = result.replace(&format!("{{{{{}}}}}", k), v);
    }
    if result != template {
        Some(result)
    } else {
        None
    }
}

fn render_request_editor(f: &mut Frame, app: &App) {
    let area = centered_rect(78, 88, f.area());
    f.render_widget(Clear, area);

    let re = &app.request_editor;
    let params = &app.params;
    let test_name = app.tests.get(re.test_idx).map(|t| t.name.as_str()).unwrap_or("?");
    let title = format!(" Edit Request — {} ", clip(test_name, 40));
    let block = Block::bordered().title(title).style(Style::new().bg(Color::Black));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let content_h = inner.height.saturating_sub(1);
    let content_rect = Rect::new(inner.x, inner.y, inner.width, content_h);
    let footer_rect = Rect::new(inner.x, inner.y + content_h, inner.width, 1);

    const LABEL_W: usize = 10; // fixed label column width
    const SEP: &str = "  ";

    let body_row = re.body_row();

    // Build visual lines; track which visual line each logical row starts on.
    let mut lines: Vec<Line> = Vec::new();
    let mut row_start: Vec<usize> = Vec::new(); // visual line where logical row i begins

    for logical_row in 0..re.row_count() {
        row_start.push(lines.len());
        let selected = logical_row == re.cursor;
        let bg = if selected {
            Color::DarkGray
        } else {
            Color::Reset
        };
        let base = Style::new().bg(bg);
        let dim = Style::new().fg(Color::DarkGray).bg(bg);

        match logical_row {
            // ── Method ────────────────────────────────────────────────────
            0 => {
                let label = Span::styled(pad_clip("Method", LABEL_W), dim);
                let val_spans = if selected && re.editing {
                    vec![Span::styled(
                        format!("{}▌", re.input),
                        base.fg(Color::Yellow),
                    )]
                } else {
                    template_spans(&re.method, params, base, bg)
                };
                lines.push(Line::from(
                    std::iter::once(label)
                        .chain(std::iter::once(Span::styled(SEP, dim)))
                        .chain(val_spans)
                        .collect::<Vec<_>>(),
                ));
                // Resolved hint when focused and not editing
                if selected
                    && !re.editing
                    && let Some(resolved) = resolve_vars(&re.method, params)
                {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(LABEL_W + SEP.len())),
                        Span::styled(format!("↳ {}", resolved), Style::new().fg(Color::DarkGray)),
                    ]));
                }
            },

            // ── URL ───────────────────────────────────────────────────────
            1 => {
                let label = Span::styled(pad_clip("URL", LABEL_W), dim);
                let val_spans = if selected && re.editing {
                    vec![Span::styled(
                        format!("{}▌", re.input),
                        base.fg(Color::Yellow),
                    )]
                } else {
                    template_spans(&re.url, params, base, bg)
                };
                lines.push(Line::from(
                    std::iter::once(label)
                        .chain(std::iter::once(Span::styled(SEP, dim)))
                        .chain(val_spans)
                        .collect::<Vec<_>>(),
                ));
                if selected
                    && !re.editing
                    && let Some(resolved) = resolve_vars(&re.url, params)
                {
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(LABEL_W + SEP.len())),
                        Span::styled(format!("↳ {}", resolved), Style::new().fg(Color::DarkGray)),
                    ]));
                }
            },

            // ── Body ──────────────────────────────────────────────────────
            r if r == body_row && re.form_fields.is_none() => {
                let label_first = pad_clip("Body", LABEL_W);
                let blank_label = " ".repeat(LABEL_W);

                if selected && re.editing {
                    // Show single-line editor
                    lines.push(Line::from(vec![
                        Span::styled(label_first, dim),
                        Span::styled(SEP, dim),
                        Span::styled(format!("{}▌", re.input), base.fg(Color::Yellow)),
                    ]));
                } else {
                    // Show full multi-line body with syntax-highlighted vars
                    let body_str = if re.body.is_empty() {
                        "(empty)"
                    } else {
                        &re.body
                    };
                    for (li, body_line) in body_str.lines().enumerate() {
                        let lbl = if li == 0 {
                            Span::styled(label_first.clone(), dim)
                        } else {
                            Span::styled(blank_label.clone(), dim)
                        };
                        let val_spans = template_spans(body_line, params, base, bg);
                        lines.push(Line::from(
                            std::iter::once(lbl)
                                .chain(std::iter::once(Span::styled(SEP, dim)))
                                .chain(val_spans)
                                .collect::<Vec<_>>(),
                        ));
                    }
                    // Resolved hint when focused and body has vars
                    if selected && let Some(resolved) = resolve_vars(&re.body, params) {
                        lines.push(Line::from(vec![
                            Span::raw(" ".repeat(LABEL_W + SEP.len())),
                            Span::styled("↳ ", Style::new().fg(Color::DarkGray)),
                        ]));
                        for hint_line in resolved.lines() {
                            lines.push(Line::from(vec![
                                Span::raw(" ".repeat(LABEL_W + SEP.len() + 2)),
                                Span::styled(
                                    hint_line.to_string(),
                                    Style::new().fg(Color::DarkGray),
                                ),
                            ]));
                        }
                    }
                }
            },

            // ── Form field[i] ─────────────────────────────────────────────
            r if re.form_fields.is_some() && r >= body_row => {
                let fi = r - body_row;
                if let Some(fields) = &re.form_fields {
                    if let Some((k, v)) = fields.get(fi) {
                        let label = Span::styled(pad_clip(&format!("{{{}}}", fi), LABEL_W), dim);

                        let key_spans: Vec<Span> = if selected && re.editing && re.edit_key {
                            vec![Span::styled(
                                format!("{}▌", re.input),
                                base.fg(Color::Yellow),
                            )]
                        } else {
                            template_spans(k, params, dim, bg)
                        };
                        let val_spans: Vec<Span> = if selected && re.editing && !re.edit_key {
                            vec![Span::styled(
                                format!("{}▌", re.input),
                                base.fg(Color::Yellow),
                            )]
                        } else {
                            template_spans(v, params, base, bg)
                        };

                        let colon = Span::styled(": ", Style::new().fg(Color::DarkGray).bg(bg));
                        lines.push(Line::from(
                            std::iter::once(label)
                                .chain(std::iter::once(Span::styled(SEP, dim)))
                                .chain(key_spans)
                                .chain(std::iter::once(colon))
                                .chain(val_spans)
                                .collect::<Vec<_>>(),
                        ));

                        if selected && !re.editing {
                            let resolved_k =
                                resolve_vars(k, params).unwrap_or_else(|| k.to_string());
                            let resolved_v =
                                resolve_vars(v, params).unwrap_or_else(|| v.to_string());
                            if resolved_k.as_str() != k.as_str()
                                || resolved_v.as_str() != v.as_str()
                            {
                                lines.push(Line::from(vec![
                                    Span::raw(" ".repeat(LABEL_W + SEP.len())),
                                    Span::styled(
                                        format!("↳ {}: {}", resolved_k, resolved_v),
                                        Style::new().fg(Color::DarkGray),
                                    ),
                                ]));
                            }
                        }

                        if selected && !re.editing {
                            let col_hint = if re.edit_key {
                                "[Tab] edit value"
                            } else {
                                "[Tab] edit key"
                            };
                            lines.push(Line::from(vec![
                                Span::raw(" ".repeat(LABEL_W + SEP.len())),
                                Span::styled(
                                    col_hint,
                                    Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                                ),
                            ]));
                        }
                    } else {
                        // Placeholder row when fields list is empty
                        let label = Span::styled(pad_clip("Form", LABEL_W), dim);
                        lines.push(Line::from(vec![
                            label,
                            Span::styled(SEP, dim),
                            Span::styled("(no fields)", Style::new().fg(Color::DarkGray).bg(bg)),
                        ]));
                        if selected {
                            lines.push(Line::from(vec![
                                Span::raw(" ".repeat(LABEL_W + SEP.len())),
                                Span::styled(
                                    "[a] add field",
                                    Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                                ),
                            ]));
                        }
                    }
                }
            },

            // ── Header[i] ─────────────────────────────────────────────────
            r => {
                let hi = r - 2;
                let (k, v) =
                    re.headers.get(hi).map(|(a, b)| (a.as_str(), b.as_str())).unwrap_or(("", ""));

                let label = Span::styled(pad_clip(&format!("[{}]", hi), LABEL_W), dim);

                // Key spans
                let key_spans: Vec<Span> = if selected && re.editing && re.edit_key {
                    vec![Span::styled(
                        format!("{}▌", re.input),
                        base.fg(Color::Yellow),
                    )]
                } else {
                    template_spans(k, params, dim, bg)
                };

                // Value spans
                let val_spans: Vec<Span> = if selected && re.editing && !re.edit_key {
                    vec![Span::styled(
                        format!("{}▌", re.input),
                        base.fg(Color::Yellow),
                    )]
                } else {
                    template_spans(v, params, base, bg)
                };

                // Show: [i]  key_spans: val_spans
                let colon = Span::styled(": ", Style::new().fg(Color::DarkGray).bg(bg));
                lines.push(Line::from(
                    std::iter::once(label)
                        .chain(std::iter::once(Span::styled(SEP, dim)))
                        .chain(key_spans)
                        .chain(std::iter::once(colon))
                        .chain(val_spans)
                        .collect::<Vec<_>>(),
                ));

                // Resolved hint when focused and not editing
                if selected && !re.editing {
                    let resolved_k = resolve_vars(k, params).unwrap_or_else(|| k.to_string());
                    let resolved_v = resolve_vars(v, params).unwrap_or_else(|| v.to_string());
                    if resolved_k != k || resolved_v != v {
                        lines.push(Line::from(vec![
                            Span::raw(" ".repeat(LABEL_W + SEP.len())),
                            Span::styled(
                                format!("↳ {}: {}", resolved_k, resolved_v),
                                Style::new().fg(Color::DarkGray),
                            ),
                        ]));
                    }
                }

                // Tab hint when focused and not editing
                if selected && !re.editing {
                    let col_hint = if re.edit_key {
                        "[Tab] edit value"
                    } else {
                        "[Tab] edit key"
                    };
                    lines.push(Line::from(vec![
                        Span::raw(" ".repeat(LABEL_W + SEP.len())),
                        Span::styled(
                            col_hint,
                            Style::new().fg(Color::DarkGray).add_modifier(Modifier::DIM),
                        ),
                    ]));
                }
            },
        }
    }

    // Scroll to keep focused row visible.
    let focused_start = row_start.get(re.cursor).copied().unwrap_or(0) as u16;
    let visible = content_rect.height as u16;
    let offset = (focused_start + 3).saturating_sub(visible);

    f.render_widget(Paragraph::new(lines).scroll((offset, 0)), content_rect);

    if let Some(msg) = &re.message {
        let msg_rect = Rect::new(inner.x, inner.y + content_h.saturating_sub(1), inner.width, 1);
        f.render_widget(
            Paragraph::new(format!("  ✓ {}", msg)).style(Style::new().fg(Color::Green)),
            msg_rect,
        );
    }

    let footer = if re.editing {
        "  [Enter] confirm  [Esc] cancel  [Backspace] del"
    } else {
        "  [jk] nav  [e/Enter] edit  [Tab] key↔val  [a] add row  [d] del  [s] save  [Esc] close"
    };
    f.render_widget(Paragraph::new(footer).style(Style::new().fg(Color::DarkGray)), footer_rect);
}

// ─── Status bar ───────────────────────────────────────────────────────────────

fn render_statusbar(f: &mut Frame, app: &App, area: Rect) {
    let (passed, failed) = app.summary();

    let (text, style) = match &app.phase {
        Phase::Empty => (
            "  No collection loaded — press [o] to open a file or directory".to_string(),
            Style::new().bg(Color::Blue).fg(Color::White),
        ),
        Phase::Idle => {
            let (p, f) = (passed, failed);
            let s = if p + f == 0 {
                format!("  {} requests  — [Enter] run selected  [r] run all", app.tests.len())
            } else {
                format!("  {} passed  {} failed  — [Enter] run selected  [r] run all", p, f)
            };
            (s, Style::new().bg(Color::DarkGray).fg(Color::White))
        },
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
    let area = centered_rect(92, 88, f.area());
    f.render_widget(Clear, area);

    let cwd_str = app.browser.cwd.display().to_string();
    let title = format!(" Open: {} ", clip(&cwd_str, area.width.saturating_sub(10) as usize));
    let outer_block = Block::bordered().title(title).style(Style::new().bg(Color::Black));
    let inner = outer_block.inner(area);
    f.render_widget(outer_block, area);

    // Reserve 1 line for footer.
    let content_h = inner.height.saturating_sub(1);
    let content_rect = Rect::new(inner.x, inner.y, inner.width, content_h);
    let footer_rect = Rect::new(inner.x, inner.y + content_h, inner.width, 1);

    // Split content horizontally: 35% file list │ 65% preview.
    let cols = Layout::horizontal([Constraint::Percentage(35), Constraint::Percentage(65)])
        .split(content_rect);
    let list_rect = cols[0];
    let preview_rect = cols[1];

    // ── File list ──────────────────────────────────────────────────────────────
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
                    " ↑  ..",
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
                        Span::styled(" ▶ ", style),
                        Span::styled(name.as_str(), style.bold()),
                        Span::styled("/", Style::new().fg(Color::DarkGray)),
                    ]))
                },
                BrowserEntry::CollectionFile(name, _, label) => ListItem::new(Line::from(vec![
                    Span::raw("   "),
                    Span::styled(name.as_str(), if selected { sel_style } else { Style::new() }),
                    Span::styled(format!(" .{}", label), Style::new().fg(Color::DarkGray)),
                ])),
            }
        })
        .collect();

    let list = List::new(items);
    let mut state = ListState::default().with_selected(Some(browser.cursor));
    f.render_stateful_widget(list, list_rect, &mut state);

    // ── Preview pane ───────────────────────────────────────────────────────────
    let preview_block =
        Block::default().borders(Borders::LEFT).style(Style::new().bg(Color::Black));
    let preview_inner = preview_block.inner(preview_rect);
    f.render_widget(preview_block, preview_rect);

    if app.browser_preview_lines.is_empty() {
        // No file selected or file couldn't be read — show hint.
        let hint = match browser.selected() {
            Some(BrowserEntry::Dir(name, _)) => {
                format!("  {} — press [Enter] to open or [Space] to load dir", name)
            },
            Some(BrowserEntry::CollectionFile(name, _, _)) => {
                format!("  {} — loading…", name)
            },
            _ => "  Select a collection file to preview its contents.".to_string(),
        };
        f.render_widget(
            Paragraph::new(hint).style(Style::new().fg(Color::DarkGray)),
            preview_inner,
        );
    } else {
        // Render with syntax highlighting.
        let selected_label = match browser.selected() {
            Some(BrowserEntry::CollectionFile(name, _, label)) => Some((*label, name.as_str())),
            _ => None,
        };
        let is_http = matches!(selected_label, Some(("http", _)) | Some(("bru", _)));

        let lines: Vec<Line> = app //
            .browser_preview_lines
            .iter()
            .map(|raw| http_preview_line(raw, is_http))
            .collect();

        f.render_widget(
            Paragraph::new(lines) //
                .wrap(Wrap { trim: false }),
            preview_inner,
        );
    }

    let footer = "  [jk] nav  [Enter] enter dir/open  [Space] open dir  [h/←/BS] back  [Esc] close";
    f.render_widget(Paragraph::new(footer).style(Style::new().fg(Color::DarkGray)), footer_rect);
}

/// Apply basic syntax colouring to one line of an .http/.bru file preview.
/// When `is_http` is false the line is returned unstyled.
fn http_preview_line(line: &str, is_http: bool) -> Line<'static> {
    if !is_http {
        return Line::from(Span::raw(line.to_string()));
    }

    // Section separator / metadata comment
    if line.starts_with("###") {
        return Line::from(Span::styled(line.to_string(), Style::new().fg(Color::DarkGray)));
    }

    // Script delimiters `> {%` / `< {%` / `%}`
    if line.trim_start().starts_with("> {%")
        || line.trim_start().starts_with("< {%")
        || line.trim_start() == "%}"
    {
        return Line::from(Span::styled(line.to_string(), Style::new().fg(Color::DarkGray)));
    }

    // HTTP method line: "METHOD URL"
    const METHODS: &[&str] = &[
        "GET", "POST", "PUT", "DELETE", "PATCH", "HEAD", "OPTIONS", "TRACE",
    ];
    for method in METHODS {
        if line.starts_with(method) && line[method.len()..].starts_with(' ') {
            let url = &line[method.len() + 1..];
            return Line::from(vec![
                Span::styled(method.to_string(), Style::new().fg(Color::Green).bold()),
                Span::raw(" "),
                Span::styled(url.to_string(), Style::new().fg(Color::Cyan)),
            ]);
        }
    }

    // Header line: "Name: value" (must not start with whitespace or `{`)
    if !line.starts_with([' ', '\t', '{', '['])
        && let Some(colon) = line.find(": ")
    {
        let key = &line[..colon];
        let val = &line[colon + 2..];
        // Sanity: key should look like a header name (no spaces)
        if !key.contains(' ') && !key.is_empty() {
            return Line::from(vec![
                Span::styled(key.to_string(), Style::new().fg(Color::DarkGray)),
                Span::styled(": ", Style::new().fg(Color::DarkGray)),
                Span::styled(val.to_string(), Style::new()),
            ]);
        }
    }

    // Default
    Line::from(Span::raw(line.to_string()))
}

// ─── Convert overlay ──────────────────────────────────────────────────────────

fn render_convert_overlay(f: &mut Frame, app: &App) {
    let area = centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

    let block = Block::bordered().title(" Export Collection ").style(Style::new().bg(Color::Black));
    let inner = block.inner(area);
    f.render_widget(block, area);

    // Split inner: formats list on top, output path + message below, footer at bottom.
    let content_h = inner.height.saturating_sub(3); // path row + message row + footer
    let content_rect = Rect::new(inner.x, inner.y, inner.width, content_h);
    let path_rect = Rect::new(inner.x, inner.y + content_h, inner.width, 1);
    let msg_rect = Rect::new(inner.x, inner.y + content_h + 1, inner.width, 1);
    let footer_rect = Rect::new(inner.x, inner.y + content_h + 2, inner.width, 1);

    // Format selection list.
    let convert = &app.convert;
    let items: Vec<ListItem> = CONVERT_FORMATS
        .iter()
        .enumerate()
        .map(|(i, (_, _, label))| {
            let selected = i == convert.format_cursor;
            let style = if selected {
                Style::new().bg(Color::DarkGray).add_modifier(Modifier::BOLD)
            } else {
                Style::new()
            };
            let prefix = if selected { "  ▶  " } else { "     " };
            ListItem::new(Line::from(vec![
                Span::styled(prefix, style),
                Span::styled(*label, style),
            ]))
        })
        .collect();
    let list = List::new(items);
    let mut state = ListState::default().with_selected(Some(convert.format_cursor));
    f.render_stateful_widget(list, content_rect, &mut state);

    // Output path row.
    let path_display = if convert.editing {
        format!("  Output: {}▌", convert.output_path)
    } else {
        format!("  Output: {}", convert.output_path)
    };
    let path_style = if convert.editing {
        Style::new().fg(Color::Yellow)
    } else {
        Style::new().fg(Color::White)
    };
    f.render_widget(Paragraph::new(path_display).style(path_style), path_rect);

    // Message row (success / error feedback).
    if let Some(msg) = &convert.message {
        let (msg_text, msg_style) = if msg.starts_with("saved") {
            (msg.as_str(), Style::new().fg(Color::Green))
        } else {
            (msg.as_str(), Style::new().fg(Color::Red))
        };
        f.render_widget(Paragraph::new(format!("  {}", msg_text)).style(msg_style), msg_rect);
    }

    // Footer.
    let footer = if convert.editing {
        "  [Enter] confirm path  [Esc] cancel edit"
    } else {
        "  [jk] select format  [e/Tab] edit path  [Enter] export  [Esc] close"
    };
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
