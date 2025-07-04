use ratatui::{
    DefaultTerminal, Frame,
    layout::{Constraint, Direction, Layout},
};
use std::{
    collections::{HashMap, VecDeque},
    fmt::Display,
};
use std::{
    error::Error,
    sync::mpsc::{SyncSender, sync_channel},
};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use tracing::{debug, error, trace, warn};

use crate::{
    ExecAgg,
    analytics::{
        reports::{ReportWriter, ServerWriter},
        requests::{DataComparator, ExecAggDiff},
    },
};

use super::chart::{Chart, MetricType};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug)]
struct App {
    current_view: View,
    prev_page: Option<Page>,
    current_page: Page,
    next_page: Option<Page>,
    metrics_order: HashMap<usize, MetricType>,
    exit: bool,
    report_writer: Arc<dyn ReportWriter>,
    rendering_queue: VecDeque<View>,
}

impl App {
    fn new(mut rendering_queue: VecDeque<View>, report_writer: Arc<dyn ReportWriter>) -> Self {
        let current_view = rendering_queue.pop_front().unwrap_or_default();
        let mut metrics_order = HashMap::new();
        metrics_order.insert(0, MetricType::TotalExecTime);
        metrics_order.insert(1, MetricType::TotalMemoryUsage);
        metrics_order.insert(2, MetricType::TotalExecCount);
        metrics_order.insert(3, MetricType::TimeAverage);

        Self {
            current_page: Page::First((MetricType::TotalExecTime, MetricType::TotalMemoryUsage)),
            next_page: Some(Page::Second((MetricType::TotalExecCount, MetricType::TimeAverage))),
            metrics_order,
            prev_page: None,
            current_view,
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
    DiffView((MetricType, ExecAggDiff)),
}

impl Display for Page {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let val = match self {
            Page::First((m1, m2)) => format!("First page: {m1}, {m2}"),
            Page::Second((m1, m2)) => format!("Second page: {m1}, {m2}"),
            Page::DiffView((m, _)) => format!("Diff view page: {m}"),
        };

        write!(f, "{val}")
    }
}

/// Contains a complete view in the screen
#[derive(Debug, Default)]
#[allow(dead_code)]
struct View {
    charts: Vec<Chart>,
    exec_agg: Option<Arc<RwLock<ExecAgg>>>,
    diff_data: Option<ExecAggDiff>,
    layout: Layout,
}

impl View {
    fn new(
        mut charts: Vec<Chart>,
        exec_agg: Option<Arc<RwLock<ExecAgg>>>,
        diff_data: Option<ExecAggDiff>,
        layout: Option<Layout>,
    ) -> Self {
        let layout = layout.unwrap_or_default();

        // Share data with the inner charts
        if let Some(exec_agg) = &exec_agg {
            for chart in &mut charts {
                chart.exec_agg = Some(exec_agg.clone());
            }
        }

        Self {
            charts,
            exec_agg,
            diff_data,
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
        // Get the page configuration for queuing it.
        let view = match &self.current_page {
            Page::First((metric1, metric2)) => {
                let c1 = Chart::new(metric1.clone());
                let c2 = Chart::new(metric2.clone());
                View::new(
                    vec![c1, c2],
                    self.current_view.exec_agg.clone(),
                    None,
                    Some(self.current_view.layout.clone()),
                )
            }
            Page::Second((metric1, metric2)) => {
                let c1 = Chart::new(metric1.clone());
                let c2 = Chart::new(metric2.clone());
                View::new(
                    vec![c1, c2],
                    self.current_view.exec_agg.clone(),
                    None,
                    Some(self.current_view.layout.clone()),
                )
            }
            Page::DiffView((metric, diff_data)) => {
                let mut chart = Chart::new(MetricType::DiffView(Box::new(metric.clone())));
                chart.diff_data = Some(diff_data.clone());
                let layout = Layout::new(Direction::Vertical, vec![Constraint::Percentage(100)]);
                View::new(vec![chart], None, Some(diff_data.clone()), Some(layout))
            }
        };

        self.rendering_queue.push_back(view);
    }

    fn handle_events(&mut self) -> std::io::Result<()> {
        // Use poll_timeout to make the event loop non-blocking
        // This allows the UI to update even without user input
        if event::poll(std::time::Duration::from_secs(1))? {
            match event::read()? {
                // it's important to check that the event is a key press event as
                // crossterm also emits key release and repeat events on Windows.
                Event::Key(key_event) if key_event.kind == KeyEventKind::Press => self.handle_key_event(key_event)?,
                _ => {}
            };
        }
        Ok(())
    }

    fn handle_key_event(&mut self, key_event: KeyEvent) -> std::io::Result<()> {
        match key_event.code {
            KeyCode::Char('q') => self.exit(),
            KeyCode::Left | KeyCode::Char('h') => {
                let subsequent_page = self.get_diff_subsequent_page(-2);
                self.change_page(-1, subsequent_page)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                let subsequent_page = self.get_diff_subsequent_page(2);
                self.change_page(1, subsequent_page)
            }
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
                let diff = receiver
                    .recv()
                    .map_err(std::io::Error::other);

                if let Ok(Some(diff)) = diff {
                    self.current_page = Page::DiffView((MetricType::TotalExecTime, diff.clone()));
                    self.next_page = Some(Page::DiffView((MetricType::TotalMemoryUsage, diff)));
                    self.prev_page = None;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn get_diff_subsequent_page(&self, offset: i16) -> Option<Page> {
        match &self.current_page {
            Page::DiffView((current_metric, data)) => {
                // Convert the metrics_order HashMap into a sorted Vec of (index, metric)
                let mut metrics: Vec<_> = self.metrics_order.iter().collect();
                metrics.sort_by_key(|&(idx, _)| idx);

                // Find the position of the current metric in the sorted list
                let pos = metrics.iter().position(|(_, m)| **m == *current_metric)?;

                // Calculate the target position with bounds checking
                let target_pos = if offset < 0 {
                    pos.checked_sub(offset.unsigned_abs() as usize)
                } else {
                    pos.checked_add(offset as usize)
                };

                // Get the metric at the target position if it exists
                target_pos
                    .and_then(|idx| metrics.get(idx))
                    .map(|(_, metric)| Page::DiffView(((**metric).clone(), data.clone())))
            }
            _ => None, // Only DiffView pages have subsequent pages
        }
    }

    fn reset_data(&self) {
        if let Some(exec_agg_ref) = self.current_view.exec_agg.clone() {
            tokio::spawn(async move {
                exec_agg_ref.write().await.agg_data.clear();
            });
        }
    }

    fn capture_and_compare_data(
        &self,
        previous_filename: String,
        new_capture_filename: String,
        sender: SyncSender<Option<ExecAggDiff>>,
    ) {
        if let Some(exec_agg_ref) = self.current_view.exec_agg.clone() {
            let report_writer = self.report_writer.clone();
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
    }

    fn capture_and_reset_data(&self, filename: String) {
        if let Some(exec_agg_ref) = self.current_view.exec_agg.clone() {
            // Clone report_writer or obtain an owned version before spawning
            let report_writer = self.report_writer.clone();
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
    }

    fn change_page(&mut self, offset: i16, subsequent_page: Option<Page>) {
        // Early return if trying to go beyond boundaries
        if (offset < 0 && self.prev_page.is_none()) || (offset > 0 && self.next_page.is_none() || offset == 0) {
            return;
        }

        // Handle negative and positive offsets safely
        if offset < 0 {
            self.next_page = Some(self.current_page.clone());
            self.current_page = self.prev_page.clone().unwrap();
            self.prev_page = subsequent_page;
        } else {
            self.prev_page = Some(self.current_page.clone());
            self.current_page = self.next_page.clone().unwrap();
            self.next_page = subsequent_page;
        }

        debug!("Changed to page {:?}", self.current_page);
    }

    fn exit(&mut self) {
        self.exit = true
    }
}

pub fn render_app(exec_agg: Arc<RwLock<ExecAgg>>) -> Result<(), Box<dyn Error>> {
    trace!("Rendering app");

    let chart1 = Chart::new(MetricType::TotalExecTime);
    let chart2 = Chart::new(MetricType::TotalExecCount);

    let charts = vec![chart1, chart2];

    let mut terminal = ratatui::init();
    let report_writer = Arc::new(ServerWriter::new());
    let mut deque = VecDeque::new();

    let layout = Layout::new(
        Direction::Vertical,
        vec![Constraint::Percentage(50), Constraint::Percentage(50)],
    );
    let view = View::new(charts, Some(exec_agg), None, Some(layout));
    deque.push_back(view);
    let app_result = App::new(deque, report_writer).run(&mut terminal);
    ratatui::restore();
    app_result?;
    Ok(())
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

        fn set_report_name(&mut self, _new_name: String) {}
    }

    #[test]
    fn test_get_diff_subsequent_page() {
        let report_writer = Arc::new(MockReportWriter);
        let mut app = App::new(VecDeque::default(), report_writer);
        let test_diff = ExecAggDiff::default();

        // Test each metric position
        for (idx, metric) in app.metrics_order.clone().iter() {
            // Set up the current page with this metric
            app.current_page = Page::DiffView((metric.clone(), test_diff.clone()));

            // Test moving forward
            let forward_result = app.get_diff_subsequent_page(2);
            let expected_forward = if idx + 2 < app.metrics_order.len() {
                app.metrics_order
                    .get(&(idx + 2))
                    .map(|m| Page::DiffView((m.clone(), test_diff.clone())))
            } else {
                None
            };

            // Direct comparison which might fail if ExecAggDiff doesn't implement PartialEq,
            // compare the page types and metrics only
            match (forward_result, expected_forward) {
                (Some(Page::DiffView((actual_m, _))), Some(Page::DiffView((expected_m, _)))) => {
                    assert_eq!(
                        actual_m, expected_m,
                        "Forward navigation failed for metric at idx {}",
                        idx
                    );
                }
                (None, None) => {
                    // Both are None, this is expected at the end of the list
                }
                _ => {
                    panic!("Unexpected result for forward navigation from idx {}", idx);
                }
            }

            // Test moving backward
            let backward_result = app.get_diff_subsequent_page(-2);
            let expected_backward = if *idx > 1 {
                app.metrics_order
                    .get(&(idx - 2))
                    .map(|m| Page::DiffView((m.clone(), test_diff.clone())))
            } else {
                None
            };

            match (backward_result, expected_backward) {
                (Some(Page::DiffView((actual_m, _))), Some(Page::DiffView((expected_m, _)))) => {
                    assert_eq!(
                        actual_m, expected_m,
                        "Backward navigation failed for metric at idx {}",
                        idx
                    );
                }
                (None, None) => {
                    // Both are None, this is expected at the start of the list
                }
                _ => {
                    panic!("Unexpected result for backward navigation from idx {}", idx);
                }
            }
        }
    }

    #[test]
    // Ensure that the page is rendered according to the last queued page.
    fn test_handle_pages() {
        let report_writer = Arc::new(MockReportWriter);
        let mut app = App::new(VecDeque::default(), report_writer);

        let pages = vec![
            Page::First((MetricType::TotalExecCount, MetricType::TotalMemoryUsage)),
            Page::Second((MetricType::TotalExecCount, MetricType::TimeAverage)),
            Page::DiffView((MetricType::TotalExecCount, ExecAggDiff::default())),
        ];

        for v in 0..3 {
            let current_page = pages[v].clone();
            app.current_page = current_page.clone();

            // Should schedule the first page view
            app.handle_pages();

            let charts = match current_page {
                Page::First((m1, m2)) | Page::Second((m1, m2)) => vec![m1, m2],
                Page::DiffView((m1, _)) => vec![MetricType::DiffView(Box::new(m1))],
            };

            for (i, chart) in app.rendering_queue.pop_front().unwrap().charts.iter().enumerate() {
                assert_eq!(chart.metric_type, charts[i]);
            }
        }
    }

    #[test]
    fn test_exit() {
        let report_writer = Arc::new(MockReportWriter);
        let mut app = App::new(VecDeque::default(), report_writer);
        app.handle_key_event(KeyCode::Char('q').into()).unwrap();

        assert!(app.exit);
    }
}
