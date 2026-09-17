use crate::jev::{level_of, Record, Usage, DIMENSIONS, MODEL, PAIN_THRESHOLD};
use crate::report::Report;
use crate::scan::Event;
use gpui::{
    div, prelude::*, px, relative, rgb, rgba, uniform_list, AnyElement, AsyncApp, Context, Div,
    Entity, FontWeight, Rgba, SharedString, Stateful, Window,
};
use std::path::PathBuf;
use tokio::sync::mpsc::UnboundedReceiver;

const BG: u32 = 0x1c1c1e;
const PANEL: u32 = 0x2c2c2e;
const RAISED: u32 = 0x3a3a3c;
const LINE: u32 = 0x38383a;
const TEXT: u32 = 0xf2f2f7;
const MUTED: u32 = 0x98989f;
const FAINT: u32 = 0x636366;
const ACCENT: u32 = 0x0a84ff;
const WARN: u32 = 0xff9f0a;
const DANGER: u32 = 0xff453a;
const OK: u32 = 0x30d158;

const MONO: &str = "Consolas";

const ROW_HEIGHT: f32 = 28.0;
const W_PAIN: f32 = 40.0;
const W_LINES: f32 = 46.0;
const W_ROLE: f32 = 106.0;
const W_CELL: f32 = 36.0;
const W_CONF: f32 = 40.0;
const GUTTER: f32 = 16.0;
const CELL_GAP: f32 = 4.0;
const COL_GAP: f32 = 10.0;

#[derive(Clone, PartialEq, Eq)]
pub enum Filter {
    All,
    Role(SharedString),
    Dimension(SharedString),
    NeedsReview,
}

impl Filter {
    fn slot(&self) -> &'static str {
        match self {
            Filter::All => "all",
            Filter::Role(_) => "role",
            Filter::Dimension(_) => "dimension",
            Filter::NeedsReview => "review",
        }
    }
}

pub struct PainPoints {
    total: usize,
    records: Vec<Record>,
    failures: Vec<(String, String)>,
    usage: Usage,
    finished: bool,
    cached: usize,
    filter: Filter,
    selected: Option<String>,
    root: PathBuf,
    out: PathBuf,
    written: Option<String>,
}

impl PainPoints {
    pub fn new(
        rx: UnboundedReceiver<Event>,
        root: PathBuf,
        out: PathBuf,
        cx: &mut Context<Self>,
    ) -> Self {
        cx.spawn(async move |this, cx: &mut AsyncApp| {
            let mut rx = rx;
            while let Some(event) = rx.recv().await {
                let applied = this.update(cx, |this: &mut Self, cx| {
                    this.apply(event);
                    cx.notify();
                });
                if applied.is_err() {
                    break;
                }
            }
        })
        .detach();

        Self {
            total: 0,
            records: Vec::new(),
            failures: Vec::new(),
            usage: Usage::default(),
            finished: false,
            cached: 0,
            filter: Filter::All,
            selected: None,
            root,
            out,
            written: None,
        }
    }

    fn apply(&mut self, event: Event) {
        match event {
            Event::Started { total } => self.total = total,
            Event::Done(record) => {
                let record = *record;
                let at = self
                    .records
                    .partition_point(|r| r.rank_key() < record.rank_key());
                self.records.insert(at, record);
            }
            Event::Failed { path, error } => self.failures.push((path, error)),
            Event::Finished { usage, cached } => {
                self.usage = usage;
                self.cached = cached;
                self.finished = true;
                self.written = Some(self.persist());
            }
        }
    }

    fn persist(&self) -> String {
        let report = Report {
            records: self.records.clone(),
            failures: self.failures.clone(),
            usage: self.usage,
            root: self.root.clone(),
        };
        match report.write(&self.out) {
            Ok((json, _)) => json.display().to_string(),
            Err(err) => format!("report failed: {err}"),
        }
    }

    fn pain_points(&self) -> usize {
        self.records
            .iter()
            .filter(|r| r.worst_score >= PAIN_THRESHOLD)
            .count()
    }

    fn hits(&self, key: &str) -> usize {
        self.records
            .iter()
            .filter(|r| r.scores.get(key) >= PAIN_THRESHOLD)
            .count()
    }

    fn roles(&self) -> Vec<(SharedString, usize)> {
        let mut counts: Vec<(SharedString, usize)> = Vec::new();
        for record in &self.records {
            match counts
                .iter_mut()
                .find(|(name, _)| name.as_ref() == record.role)
            {
                Some((_, n)) => *n += 1,
                None => counts.push((SharedString::from(record.role.clone()), 1)),
            }
        }
        counts.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        counts
    }

    fn visible(&self) -> Vec<&Record> {
        self.records
            .iter()
            .filter(|r| match &self.filter {
                Filter::All => true,
                Filter::Role(value) => r.role == value.as_ref(),
                Filter::Dimension(key) => r.scores.get(key.as_ref()) >= PAIN_THRESHOLD,
                Filter::NeedsReview => r.needs_review,
            })
            .collect()
    }

    fn current(&self) -> Option<&Record> {
        let path = self.selected.as_ref()?;
        self.records.iter().find(|r| &r.path == path)
    }

    fn empty_message(&self) -> &'static str {
        if self.records.is_empty() && !self.finished {
            "reading the repository"
        } else if self.records.is_empty() {
            "nothing was classified"
        } else {
            "no file matches this filter"
        }
    }
}

fn severity(score: f32) -> u32 {
    if score >= 2.5 {
        DANGER
    } else if score >= PAIN_THRESHOLD {
        WARN
    } else if score >= 1.0 {
        MUTED
    } else {
        FAINT
    }
}

fn tint(color: u32, alpha: u32) -> Rgba {
    rgba((color << 8) | alpha)
}

fn badge(text: impl Into<SharedString>, color: u32) -> impl IntoElement {
    div()
        .px(px(6.))
        .py(px(2.))
        .rounded(px(6.))
        .bg(tint(color, 0x24))
        .text_color(rgb(color))
        .text_size(px(11.))
        .child(text.into())
}

fn number(value: String, width: f32, color: u32, size: f32) -> impl IntoElement {
    div()
        .w(px(width))
        .flex_none()
        .flex()
        .justify_end()
        .font_family(MONO)
        .text_size(px(size))
        .text_color(rgb(color))
        .child(value)
}

fn heat(score: f32) -> impl IntoElement {
    let color = severity(score);
    div()
        .w(px(W_CELL))
        .h(px(20.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .when(score >= 1.0, |el| el.bg(tint(color, 0x24)))
        .font_family(MONO)
        .text_size(px(11.))
        .text_color(rgb(color))
        .child(format!("{score:.1}"))
}

fn meter(fraction: f32, color: u32, height: f32) -> impl IntoElement {
    div()
        .h(px(height))
        .w_full()
        .rounded(px(height / 2.))
        .bg(rgb(LINE))
        .child(
            div()
                .h_full()
                .w(relative(fraction.clamp(0., 1.)))
                .rounded(px(height / 2.))
                .bg(rgb(color)),
        )
}

fn label(text: impl Into<SharedString>) -> impl IntoElement {
    div()
        .text_size(px(10.))
        .text_color(rgb(FAINT))
        .child(text.into())
}

fn stat(name: &'static str, value: String) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(label(name))
        .child(
            div()
                .font_family(MONO)
                .text_size(px(13.))
                .text_color(rgb(TEXT))
                .child(value),
        )
}

fn column(name: &'static str, width: f32, end: bool) -> impl IntoElement {
    div()
        .w(px(width))
        .flex_none()
        .flex()
        .when(end, |el| el.justify_end())
        .text_size(px(10.))
        .text_color(rgb(FAINT))
        .child(name)
}

impl PainPoints {
    fn header(&self) -> impl IntoElement {
        let done = self.records.len() + self.failures.len();
        let fraction = if self.total == 0 {
            0.
        } else {
            done as f32 / self.total as f32
        };
        div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .px(px(GUTTER))
            .py(px(14.))
            .bg(rgb(PANEL))
            .border_b(px(1.))
            .border_color(rgb(LINE))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(28.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(
                                div()
                                    .text_size(px(16.))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(rgb(TEXT))
                                    .child("architecture pain points"),
                            )
                            .child(label(SharedString::from(
                                self.root.to_string_lossy().to_string(),
                            ))),
                    )
                    .child(div().flex_grow())
                    .child(stat("files", format!("{done} / {}", self.total)))
                    .child(stat("pain points", self.pain_points().to_string()))
                    .child(stat("reused", self.cached.to_string()))
                    .child(stat(
                        "tokens in / out",
                        format!("{} / {}", self.usage.input_tokens, self.usage.output_tokens),
                    ))
                    .child(stat("failed", self.failures.len().to_string()))
                    .child(badge(MODEL, ACCENT)),
            )
            .when(!self.finished, |el| {
                el.child(meter(fraction, ACCENT, 2.))
            })
            .when_some(self.written.clone(), |el, path| {
                el.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(
                            div()
                                .w(px(5.))
                                .h(px(5.))
                                .rounded(px(3.))
                                .bg(rgb(OK))
                                .flex_none(),
                        )
                        .child(label(SharedString::from(path))),
                )
            })
    }

    fn filter_button(
        &self,
        name: SharedString,
        count: usize,
        share: f32,
        filter: Filter,
        color: u32,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let selected = self.filter == filter;
        div()
            .id(SharedString::from(format!("{}-{name}", filter.slot())))
            .flex()
            .flex_col()
            .gap(px(4.))
            .pl(px(8.))
            .pr(px(8.))
            .py(px(6.))
            .rounded(px(8.))
            .cursor_pointer()
            .when(selected, |el| el.bg(rgb(RAISED)))
            .hover(|el| el.bg(rgb(LINE)))
            .active(|el| el.bg(rgb(PANEL)))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(
                        div()
                            .flex_grow()
                            .overflow_hidden()
                            .text_size(px(12.))
                            .text_color(rgb(if selected || count > 0 { TEXT } else { MUTED }))
                            .child(name),
                    )
                    .child(number(
                        count.to_string(),
                        24.,
                        if count > 0 { color } else { FAINT },
                        11.,
                    )),
            )
            .child(meter(share, if count > 0 { color } else { LINE }, 2.))
            .on_click(cx.listener(move |this, _event, _window, cx| {
                this.filter = if this.filter == filter {
                    Filter::All
                } else {
                    filter.clone()
                };
                cx.notify();
            }))
    }

    fn heading(title: &'static str) -> impl IntoElement {
        div().px(px(8.)).pt(px(14.)).pb(px(6.)).child(label(title))
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let total = self.records.len().max(1) as f32;
        let review = self.records.iter().filter(|r| r.needs_review).count();
        let mut column = div()
            .w(px(236.))
            .flex_none()
            .h_full()
            .bg(rgb(PANEL))
            .border_r(px(1.))
            .border_color(rgb(LINE))
            .overflow_hidden()
            .flex()
            .flex_col()
            .px(px(8.))
            .child(Self::heading("PAIN BY DIMENSION"));

        for dimension in DIMENSIONS {
            let count = self.hits(dimension.key);
            column = column.child(self.filter_button(
                SharedString::from(dimension.label),
                count,
                count as f32 / total,
                Filter::Dimension(SharedString::from(dimension.key)),
                DANGER,
                cx,
            ));
        }

        column = column.child(Self::heading("ROLE"));
        for (name, count) in self.roles() {
            column = column.child(self.filter_button(
                name.clone(),
                count,
                count as f32 / total,
                Filter::Role(name),
                ACCENT,
                cx,
            ));
        }

        column
            .child(Self::heading("FLAGS"))
            .child(self.filter_button(
                "low confidence".into(),
                review,
                review as f32 / total,
                Filter::NeedsReview,
                WARN,
                cx,
            ))
    }
}

impl PainPoints {
    fn column_header(&self) -> impl IntoElement {
        let mut header = div()
            .h(px(26.))
            .flex()
            .items_center()
            .gap(px(COL_GAP))
            .px(px(GUTTER))
            .bg(rgb(BG))
            .border_b(px(1.))
            .border_color(rgb(LINE))
            .child(column("PAIN", W_PAIN, true))
            .child(div().flex_grow().child(label("FILE")))
            .child(column("LINES", W_LINES, true))
            .child(column("ROLE", W_ROLE, false));

        let mut cells = div().flex().gap(px(CELL_GAP));
        for dimension in DIMENSIONS {
            cells = cells.child(
                div()
                    .w(px(W_CELL))
                    .flex_none()
                    .flex()
                    .justify_center()
                    .child(label(dimension.short)),
            );
        }

        header.child(cells).child(column("CONF", W_CONF, true))
    }

    fn detail(&self) -> AnyElement {
        let panel = div()
            .id("detail")
            .w(px(360.))
            .flex_none()
            .h_full()
            .bg(rgb(PANEL))
            .border_l(px(1.))
            .border_color(rgb(LINE))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(14.))
            .px(px(GUTTER))
            .py(px(14.));

        let Some(record) = self.current() else {
            let mut summary = panel
                .child(label("THIS REPOSITORY"))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(rgb(TEXT))
                        .child(format!(
                            "{} of {} files carry a pain point",
                            self.pain_points(),
                            self.records.len()
                        )),
                );
            for dimension in DIMENSIONS {
                let count = self.hits(dimension.key);
                summary = summary.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(4.))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .child(
                                    div()
                                        .flex_grow()
                                        .text_size(px(11.))
                                        .text_color(rgb(if count > 0 { TEXT } else { MUTED }))
                                        .child(dimension.label),
                                )
                                .child(number(
                                    count.to_string(),
                                    28.,
                                    if count > 0 { DANGER } else { FAINT },
                                    11.,
                                )),
                        )
                        .child(label(dimension.source)),
                );
            }
            return summary
                .child(
                    div()
                        .pt(px(4.))
                        .text_size(px(11.))
                        .text_color(rgb(MUTED))
                        .child("Select a file to see which level each score landed on."),
                )
                .into_any_element();
        };

        let mut ranked: Vec<(&crate::jev::Dimension, f32)> = DIMENSIONS
            .iter()
            .map(|d| (d, record.scores.get(d.key)))
            .collect();
        ranked.sort_by(|a, b| b.1.total_cmp(&a.1));

        let mut detail = panel
            .child(label("SELECTED FILE"))
            .child(
                div()
                    .text_size(px(12.))
                    .text_color(rgb(TEXT))
                    .child(SharedString::from(record.path.clone())),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .child(badge(SharedString::from(record.role.clone()), ACCENT))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(format!("{} lines", record.lines)),
                    )
                    .child(div().flex_grow())
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(if record.needs_review { WARN } else { MUTED }))
                            .child(if record.needs_review {
                                "low confidence".to_string()
                            } else {
                                format!("{:.0}% confident", record.role_confidence * 100.)
                            }),
                    ),
            );

        for (dimension, score) in ranked {
            let color = severity(score);
            detail = detail.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .p(px(12.))
                    .rounded(px(10.))
                    .bg(rgb(RAISED))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(
                                div()
                                    .flex_grow()
                                    .text_size(px(12.))
                                    .text_color(rgb(TEXT))
                                    .child(dimension.label),
                            )
                            .child(number(format!("{score:.1}"), 28., color, 12.)),
                    )
                    .child(meter(score / 3., color, 3.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(MUTED))
                            .child(dimension.levels[level_of(score)]),
                    )
                    .child(label(dimension.url)),
            );
        }

        detail.into_any_element()
    }
}

fn row(index: usize, record: &Record, selected: bool) -> Stateful<Div> {
    let mut cells = div().flex().gap(px(CELL_GAP));
    for value in record.scores.values() {
        cells = cells.child(heat(value));
    }

    let content = div()
        .flex_grow()
        .flex()
        .items_center()
        .gap(px(COL_GAP))
        .child(number(
            format!("{:.1}", record.worst_score),
            W_PAIN,
            severity(record.worst_score),
            12.,
        ))
        .child(
            div()
                .flex_grow()
                .overflow_hidden()
                .text_size(px(12.))
                .text_color(rgb(TEXT))
                .child(SharedString::from(record.path.clone())),
        )
        .child(number(record.lines.to_string(), W_LINES, FAINT, 11.))
        .child(
            div()
                .w(px(W_ROLE))
                .flex_none()
                .text_size(px(11.))
                .text_color(rgb(MUTED))
                .child(SharedString::from(record.role.clone())),
        )
        .child(cells)
        .child(number(
            if record.needs_review {
                "low".to_string()
            } else {
                format!("{:.0}%", record.role_confidence * 100.)
            },
            W_CONF,
            if record.needs_review { WARN } else { FAINT },
            11.,
        ));

    div()
        .id(("row", index))
        .h(px(ROW_HEIGHT))
        .flex()
        .flex_col()
        .cursor_pointer()
        .pr(px(GUTTER))
        .pl(px(GUTTER - 2.))
        .border_l(px(2.))
        .border_color(if selected { rgb(ACCENT) } else { rgba(0) })
        .when(selected, |el| el.bg(rgb(RAISED)))
        .hover(|el| el.bg(rgb(PANEL)))
        .child(content)
        .child(div().h(px(1.)).w_full().flex_none().bg(tint(LINE, 0x80)))
}

impl Render for PainPoints {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let visible: Vec<Record> = self.visible().into_iter().cloned().collect();
        let count = visible.len();
        let empty = self.empty_message();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(BG))
            .text_color(rgb(TEXT))
            .font_family("Segoe UI")
            .child(self.header())
            .child(
                div()
                    .flex_grow()
                    .flex()
                    .overflow_hidden()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .flex_grow()
                            .flex()
                            .flex_col()
                            .overflow_hidden()
                            .child(self.column_header())
                            .when(count == 0, |el| {
                                el.child(
                                    div()
                                        .flex_grow()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .text_size(px(12.))
                                        .text_color(rgb(MUTED))
                                        .child(empty),
                                )
                            })
                            .when(count > 0, |el| {
                                el.child(
                                    uniform_list(
                                        "records",
                                        count,
                                        cx.processor(move |this, range: std::ops::Range<usize>, _window, cx| {
                                            range
                                                .map(|i| {
                                                    let record = &visible[i];
                                                    let path = record.path.clone();
                                                    let chosen =
                                                        this.selected.as_deref() == Some(path.as_str());
                                                    row(i, record, chosen)
                                                        .on_click(cx.listener(move |this, _event, _window, cx| {
                                                            this.selected = if this.selected.as_deref()
                                                                == Some(path.as_str())
                                                            {
                                                                None
                                                            } else {
                                                                Some(path.clone())
                                                            };
                                                            cx.notify();
                                                        }))
                                                        .into_any_element()
                                                })
                                                .collect()
                                        }),
                                    )
                                    .flex_grow(),
                                )
                            }),
                    )
                    .child(self.detail()),
            )
    }
}

pub fn open(
    rx: UnboundedReceiver<Event>,
    root: PathBuf,
    out: PathBuf,
    cx: &mut gpui::App,
) -> Entity<PainPoints> {
    let bounds = gpui::Bounds::centered(None, gpui::size(px(1440.), px(820.)), cx);
    let window = cx
        .open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(bounds)),
                titlebar: Some(gpui::TitlebarOptions {
                    title: Some("architecture pain points".into()),
                    ..Default::default()
                }),
                ..Default::default()
            },
            |_window, cx| cx.new(|cx| PainPoints::new(rx, root, out, cx)),
        )
        .expect("failed to open window");
    cx.activate(true);
    window.entity(cx).expect("window closed")
}
