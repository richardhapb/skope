use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
};
use std::collections::VecDeque;
use std::fmt::Display;
use std::sync::mpsc::{SyncSender, sync_channel};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use tracing::{debug, error, info, trace, warn};

use crate::{
    ExecAgg,
    analytics::{
        reports::{DefaultWriter, ReportWriter},
        requests::ExecAggDiff,
    },
};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug)]
struct App {
    current_view: View,
    pages: Vec<Page>,
    current_page: usize,
    exit: bool,
    report_writer: Arc<dyn ReportWriter>,
    rendering_queue: VecDeque<View>,
}

impl App {
    fn new(mut rendering_queue: VecDeque<View>, report_writer: Arc<dyn ReportWriter>) -> Self {
        let current_view = rendering_queue.pop_front().unwrap_or_default();

        Self {
            pages: vec![
                Page::First((MetricType::TotalExecTime, MetricType::TotalMemoryUsage)),
                Page::Second((MetricType::TotalExecCount, MetricType::TimeAverage)),
            ],
            current_view,
            current_page: 0,
            exit: false,
            report_writer,
            rendering_queue,
        }
    }
}

#[derive(Debug, Clone)]
enum Page {
    First((MetricType, MetricType)),
    Second((MetricType, MetricType)),
}

#[derive(Debug, Default, Clone)]
enum MetricType {
    #[default]
    TotalExecTime,
    TotalMemoryUsage,
    TotalExecCount,
    TimeAverage,
}

/// Contains a complete view in the screen
#[derive(Debug, Default)]
struct View {
    charts: Vec<Chart>,
    exec_agg: Arc<RwLock<ExecAgg>>,
    layout: Layout,
}

impl View {
    fn new(mut charts: Vec<Chart>, exec_agg: Arc<RwLock<ExecAgg>>, layout: Option<Layout>) -> Self {
        let layout = layout.unwrap_or_default();

        // Share data with the inner charts
        for chart in &mut charts {
            chart.exec_agg = Some(exec_agg.clone());
        }

        Self {
            charts,
            exec_agg,
            layout,
        }
    }

    fn draw(&self, frame: &mut Frame) {
        // Charts should be the same size than the layout splits
        for (i, layout) in self.layout.split(frame.area()).iter().enumerate() {
            if let Some(chart) = self.charts.get(i) {
                frame.render_widget(chart, *layout);
            } else {
                warn!("There are more layouts than charts.");
            }
        }
    }
}

#[derive(Debug, Default, Clone)]
struct Chart {
    metric_type: MetricType,
    exec_agg: Option<Arc<RwLock<ExecAgg>>>,
}

impl Widget for &Chart {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        // Use a blocking operation to safely wait for the data.
        // This approach ensures that the data is retrieved and waits for it
        // if it is not available, instead of using try_read in multiple places.
        // Also, the fallback with empty values is unexpected here. It is important
        // consider that the [`ExecAgg`] can be expensive if the data is large.
        // In this case, the data only stores aggregate data and "groups" it into a few fields.
        let exec_agg_data = tokio::task::block_in_place(|| {
            let rt = tokio::runtime::Handle::current();
            rt.block_on(async { 
                if let Some(exec_agg) = self.exec_agg.clone() {
                    exec_agg.read().await.clone()
                } else {
                    // This should not happen because
                    // the data is initialized in each view
                    warn!("Empty data in exec_agg for Chart rendering");
                    ExecAgg::default()
                }
            })
        });

        let title = Line::from(format!("{}", self.metric_type)).bold();
        let block = Block::bordered()
            .title(title.centered())
            .border_set(border::ROUNDED);

        // Create a block-sized area within the component
        let inner_area = block.inner(area);
        // Render the block to the buffer
        block.render(area, buf);

        // Split the inner area for the two paragraphs
        let inner_layout = Layout::new(
            Direction::Vertical,
            vec![Constraint::Percentage(90), Constraint::Percentage(10)],
        )
        .split(inner_area);

        let data = exec_agg_data.agg_data;

        // Label for the x-axis
        Paragraph::new(Text::from(vec![Line::from(
            "Applications".to_string().yellow(),
        )]))
        .centered()
        .render(inner_layout[1], buf);

        // Early return if no data to display
        if data.is_empty() {
            info!("No data to render, skipping chart");
            return;
        }

        let gap: u16 = 2; // gap between bars

        let data_len = data.len();
        debug!("Rendering chart with {} data points", data_len);

        let bars = if data_len == 0 {
            HashMap::new()
        } else {
            // Find the maximum execution time for scaling
            let max_time = data
                .values()
                .map(|d| match self.metric_type {
                    MetricType::TotalExecTime => d.total_exec_time,
                    MetricType::TotalMemoryUsage => d.total_memory_usage,
                    MetricType::TotalExecCount => d.total_execs as f32,
                    MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                })
                .fold(0.0, f32::max);

            // Calculate appropriate bar width to use full space
            let bar_width = inner_layout[0].width / data_len as u16;

            let mut map = HashMap::new();

            for (i, d) in data.values().enumerate() {
                let val = match self.metric_type {
                    MetricType::TotalExecTime => d.total_exec_time,
                    MetricType::TotalMemoryUsage => d.total_memory_usage,
                    MetricType::TotalExecCount => d.total_execs as f32,
                    MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                };

                // Calculate scaled height - use 70% of available height for max value
                let height =
                    ((val as f64 / max_time as f64) * inner_layout[0].height as f64 * 0.7) as u16;
                // Ensure minimum visible height
                let height = std::cmp::max(height, 1);
                // Ensure height inner bound
                let height = std::cmp::min(height, 38);

                // Position at bottom of chart area
                let y = inner_layout[0].height.saturating_sub(height + 3); // 3 = Keep space for labels

                // X position with proper offset from inner_layout, gap allows centering the
                // bars properly
                let x = inner_layout[0].x + (i as u16 * bar_width) + gap;

                debug!(%val, "Capturing");

                map.insert(
                    (d.name.clone(), (val * 100.0).round() as u32),
                    Rect::new(x, inner_layout[0].y + y, bar_width - gap, height),
                );
            }
            map
        };

        // Rendering the charts and its labels
        for (values, bar) in bars {
            debug!(
                "Rendering bar at x={}, y={}, width={}, height={}",
                bar.x, bar.y, bar.width, bar.height
            );

            let mut lines = vec![];

            // Fill the bar
            for _ in 0..bar.height {
                // Width must account for buffer boundaries
                let safe_width = std::cmp::min(
                    bar.width as usize,
                    // Prevent potential buffer overflow by limiting width
                    if bar.x + bar.width <= area.width {
                        bar.width as usize
                    } else {
                        (area.width - bar.x) as usize
                    },
                );
                if safe_width > 0 {
                    let text = "█".repeat(safe_width);
                    let text = match self.metric_type {
                        MetricType::TotalExecCount | MetricType::TotalMemoryUsage => text.yellow(),
                        MetricType::TotalExecTime | MetricType::TimeAverage => text.blue(),
                    };
                    let line = Line::from(vec![" ".into(), text, " ".into()]);
                    lines.push(line.centered());
                }
            }

            let bar_w = Text::from(lines);

            debug!(?values, "Rendering values");

            let mut name_clone = values.0.clone();

            // If text is bigger than max, break line
            if name_clone.len() > bar.width.into() {
                let mut last_line = name_clone
                    .get(bar.width as usize..)
                    .unwrap_or("")
                    .to_string();
                last_line.truncate(bar.width.into());
                let name_rect = Rect::new(bar.x, bar.y + bar.height + 1, bar.width, 1);
                Paragraph::new(Line::from(last_line).red().centered()).render(name_rect, buf);
            }

            // Print the line below or single line
            name_clone.truncate(bar.width.into());
            let name_rect = Rect::new(bar.x, bar.y + bar.height, bar.width, 1);
            Paragraph::new(Line::from(name_clone).red().centered()).render(name_rect, buf);

            // Print the value on the top of the bar
            let value_rect = Rect::new(bar.x, bar.y - 2, bar.width, 1);
            Paragraph::new(
                Line::from(format!("{:.2}", values.1 as f32 / 100.0))
                    .yellow()
                    .centered(),
            )
            .render(value_rect, buf);

            Paragraph::new(bar_w.clone()).render(bar, buf);
        }
    }
}

impl Display for MetricType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let val = match self {
            MetricType::TotalExecTime => "Total Execution Time (ms)",
            MetricType::TotalMemoryUsage => "Total Memory Usage (mb)",
            MetricType::TotalExecCount => "Total executions",
            MetricType::TimeAverage => "Average Time Per Execution (ms)",
        };

        write!(f, "{}", val)
    }
}

impl Chart {
    fn new(metric_type: MetricType) -> Self {
        Self {
            metric_type,
            exec_agg: None,
        }
    }
}

impl App {
    fn run(&mut self, terminal: &mut DefaultTerminal) -> std::io::Result<()> {
        while !self.exit {
            // If there is a view in queue render it, otherwise
            // render the current view
            terminal.draw(|frame| {
                if let Some(view) = self.rendering_queue.pop_front() {
                    view.draw(frame);
                } else {
                    self.current_view.draw(frame);
                }
            })?;
            self.handle_events()?;
            self.handle_pages();
        }

        Ok(())
    }

    fn handle_pages(&mut self) {
        // Get the current page configuration
        if let Some(page) = self.pages.get(self.current_page).cloned() {
            if self.current_view.charts.len() != 2 {
                error!("Only two charts are allowed");
                return;
            }

            let view = match page {
                Page::First((metric1, metric2)) => {
                    let c1 = Chart::new(metric1);
                    let c2 = Chart::new(metric2);
                    View::new(
                        vec![c1, c2],
                        self.current_view.exec_agg.clone(),
                        Some(self.current_view.layout.clone()),
                    )
                }
                Page::Second((metric1, metric2)) => {
                    let c1 = Chart::new(metric1);
                    let c2 = Chart::new(metric2);
                    View::new(
                        vec![c1, c2],
                        self.current_view.exec_agg.clone(),
                        Some(self.current_view.layout.clone()),
                    )
                }
            };

            self.rendering_queue.push_back(view);
        }
    }

    fn handle_events(&mut self) -> std::io::Result<()> {
        // Use poll_timeout to make the event loop non-blocking
        // This allows the UI to update even without user input
        if event::poll(std::time::Duration::from_secs(1))? {
            match event::read()? {
                // it's important to check that the event is a key press event as
                // crossterm also emits key release and repeat events on Windows.
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => {
                    self.handle_key_event(key_event)
                }
                _ => {}
            };
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) {
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Left | KeyCode::Char('h') => self.change_page(-1),
            KeyCode::Right | KeyCode::Char('l') => self.change_page(1),
            KeyCode::Char('r') => {
                self.reset_data();
            }
            // Capture a new profile data with the current data, clear the dashboard, also wait until
            // `s` is pressed, then finalize the captures and render the difference.
            KeyCode::Char('c') => {
                // TODO: I need to implement the SQLite storage
                // Capture to database the data, for now we capturing it to a file
                let filename = "capture1.json";
                let temp = std::env::temp_dir();
                let filepath = temp.join(filename);
                self.capture_and_reset_data(filepath.to_str().unwrap_or(filename).to_string());
            }
            KeyCode::Char('s') => {
                // Here i need to handle the rendering of the comparation
                let filename1 = "capture1.json";
                let filename2 = "capture2.json";
                let temp = std::env::temp_dir();
                let filepath1 = temp.join(filename1);
                let filepath2 = temp.join(filename2);

                let (sender, receiver) = sync_channel::<Option<ExecAggDiff>>(1);
                self.capture_and_compare_data(
                    filepath1.to_str().unwrap_or(filename1).to_string(),
                    filepath2.to_str().unwrap_or(filename2).to_string(),
                    sender,
                );
                let diff = receiver.recv();

                info!(?diff);
            }
            _ => {}
        }
    }

    fn reset_data(&self) {
        let exec_agg_ref = self.current_view.exec_agg.clone();
        tokio::spawn(async move {
            exec_agg_ref.write().await.agg_data.clear();
        });
    }

    fn capture_and_compare_data(
        &self,
        previous_filename: String,
        new_capture_filename: String,
        sender: SyncSender<Option<ExecAggDiff>>,
    ) {
        let report_writer = self.report_writer.clone();
        let exec_agg_ref = self.current_view.exec_agg.clone();
        tokio::spawn(async move {
            let exec_agg = { exec_agg_ref.read().await.clone() };
            if let Err(e) = report_writer.generate_report(&exec_agg, Some(&new_capture_filename)) {
                error!(%e, "Error capturing file; skipping data clearance.");
                return;
            }

            // TODO: Handle the errors
            match exec_agg.compare_files(&previous_filename, &new_capture_filename) {
                Ok(diff) => {
                    sender.send(Some(diff)).unwrap();
                }
                Err(e) => {
                    error!(%e, "Error comparing files");
                    sender.send(None).unwrap();
                }
            }
        });
    }

    fn capture_and_reset_data(&self, filename: String) {
        // Clone report_writer or obtain an owned version before spawning
        let report_writer = self.report_writer.clone();
        let exec_agg_ref = self.current_view.exec_agg.clone();
        tokio::spawn(async move {
            let (exec_agg, exec_agg_ref) = {
                let exec_agg_clone = exec_agg_ref.read().await.clone();
                (exec_agg_clone, exec_agg_ref)
            };
            if let Err(e) = report_writer.generate_report(&exec_agg, Some(&filename)) {
                error!(%e, "Error capturing file; skipping data clearance.");
                return;
            }
            exec_agg_ref.write().await.agg_data.clear();
        });
    }

    fn change_page(&mut self, offset: i16) {
        // Early return if trying to go beyond boundaries
        if (offset < 0 && self.current_page == 0)
            || (offset > 0 && self.current_page >= self.pages.len().saturating_sub(1))
        {
            return;
        }

        // Handle negative and positive offsets safely
        if offset < 0 {
            self.current_page = self.current_page.saturating_sub(-offset as usize);
        } else {
            self.current_page = self.current_page.saturating_add(offset as usize);
        }

        debug!("Changed to page {}", self.current_page);
    }

    fn exit(&mut self) {
        self.exit = true
    }
}

pub fn render_app(exec_agg: Arc<RwLock<ExecAgg>>) -> std::io::Result<()> {
    trace!("Rendering app");

    let chart1 = Chart::new(MetricType::TotalExecTime);
    let chart2 = Chart::new(MetricType::TotalExecCount);

    let charts = vec![chart1, chart2];

    let mut terminal = ratatui::init();
    let report_writer = Arc::new(DefaultWriter::new());
    let mut deque = VecDeque::new();

    let layout = Layout::new(
        Direction::Vertical,
        vec![Constraint::Percentage(50), Constraint::Percentage(50)],
    );
    let view = View::new(charts, exec_agg, Some(layout));
    deque.push_back(view);
    let app_result = App::new(deque, report_writer).run(&mut terminal);
    ratatui::restore();
    app_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analytics::reports::Reportable;

    #[derive(Debug)]
    struct MockReportWriter;

    impl ReportWriter for MockReportWriter {
        fn write_reports(&self, _reportables: Vec<Box<dyn Reportable>>) -> std::io::Result<()> {
            Ok(())
        }

        fn get_iterations_threshold(&self) -> usize {
            0
        }

        fn set_iterations_threshold(&mut self, _: usize) {}
    }

    #[test]
    fn test_exit() {
        let report_writer = Arc::new(MockReportWriter);
        let mut app = App::new(VecDeque::default(), report_writer);
        app.handle_key_event(KeyCode::Char('q').into());

        assert!(app.exit);
    }
}
