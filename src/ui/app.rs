use ratatui::{
    DefaultTerminal, Frame,
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
};
use std::fmt::Display;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use tracing::{debug, error, info, trace};

use crate::ExecAgg;
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::sync::RwLock;

#[derive(Debug)]
struct App {
    pages: Vec<Page>,
    current_page: usize,
    exit: bool,
}

impl Default for App {
    fn default() -> Self {
        Self {
            pages: vec![
                Page::First((MetricType::TotalExecTime, MetricType::TotalMemoryUsage)),
                Page::Second((MetricType::TotalExecCount, MetricType::TimeAverage)),
            ],
            current_page: 0,
            exit: false,
        }
    }
}

#[derive(Debug, Default)]
struct Chart {
    exec_agg: ExecAgg,
    metric_type: MetricType,
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

#[derive(Debug, Default)]
struct ChartWidget(Arc<RwLock<Chart>>);

impl Widget for ChartWidget {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
        let title = match self.0.try_read() {
            Ok(chart) => Line::from(format!("{}", chart.metric_type)).bold(),
            Err(_) => Line::from("Chart (Loading...)").bold(),
        };
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

        let data = match self.0.try_read() {
            Ok(guard) => {
                info!(
                    "Rendering chart with {} data items",
                    guard.exec_agg.exec_data.len()
                );
                guard.exec_agg.clone()
            }
            Err(_) => {
                error!("Error reading data in chart");
                ExecAgg::default()
            }
        };

        // Label for the x-axis
        Paragraph::new(Text::from(vec![Line::from(
            "Applications".to_string().yellow(),
        )]))
        .centered()
        .render(inner_layout[1], buf);

        // Early return if no data to display
        if data.exec_data.is_empty() {
            info!("No data to render, skipping chart");
            return;
        }

        debug!("Rendering chart with {} data points", data.exec_data.len());

        let gap: u16 = 2; // gap between bars
        let data_len = data.exec_data.len();

        let bars = if data_len == 0 {
            HashMap::new()
        } else {
            // Find the maximum execution time for scaling
            let max_time = data
                .exec_data
                .values()
                .map(|d| {
                    match self.0.try_read() {
                        Ok(chart) => match chart.metric_type {
                            MetricType::TotalExecTime => d.total_exec_time,
                            MetricType::TotalMemoryUsage => d.total_memory_usage,
                            MetricType::TotalExecCount => d.total_execs as f32,
                            MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                        },
                        Err(_) => 0.0, // Default value when lock can't be acquired
                    }
                })
                .fold(0.0, f32::max);

            // Calculate appropriate bar width to use full space
            let bar_width = inner_layout[0].width / data_len as u16;

            let mut map = HashMap::new();

            for (i, d) in data.exec_data.values().enumerate() {
                let val = match self.0.try_read() {
                    Ok(chart) => match chart.metric_type {
                        MetricType::TotalExecTime => d.total_exec_time,
                        MetricType::TotalMemoryUsage => d.total_memory_usage,
                        MetricType::TotalExecCount => d.total_execs as f32,
                        MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                    },
                    Err(_) => 0.0, // Default value when lock can't be acquired
                };

                // Calculate scaled height - use 80% of available height for max value
                let height =
                    ((val as f64 / max_time as f64) * inner_layout[0].height as f64 * 0.8) as u16;
                // Ensure minimum visible height
                let height = std::cmp::max(height, 1);

                // Position at bottom of chart area
                let y = inner_layout[0].height.saturating_sub(height + 3); // 3 = Keep space for labels

                // X position with proper offset from inner_layout, gap * 2 allows centering the
                // bars properly
                let x = inner_layout[0].x + (i as u16 * bar_width) + gap;

                debug!(%val, "Capturing");

                map.insert(
                    (d.name.clone(), (val * 100.0).round() as u32),
                    Rect::new(x, inner_layout[0].y + y, bar_width, height),
                );
            }
            map
        };

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
                    let text = match self.0.try_read() {
                        Ok(chart) => match chart.metric_type {
                            MetricType::TotalExecCount | MetricType::TotalMemoryUsage => {
                                text.yellow()
                            }
                            MetricType::TotalExecTime | MetricType::TimeAverage => text.blue(),
                        },
                        Err(_) => text.yellow(),
                    };
                    let line = Line::from(vec![" ".into(), text, " ".into()]);
                    lines.push(line.centered());
                }
            }

            let bar_w = Text::from(lines);

            debug!(?values, "Rendering values");

            let max_width = bar.width - 2;
            let mut name_clone = values.0.clone();

            // If text is bigger than max, break line
            if name_clone.len() > max_width.into() {
                let mut last_line = name_clone
                    .get(max_width as usize..)
                    .unwrap_or("")
                    .to_string();
                last_line.truncate(max_width.into());
                let name_rect = Rect::new(bar.x, bar.y + bar.height + 1, bar.width, 1);
                Paragraph::new(Line::from(last_line).red().centered()).render(name_rect, buf);
            }

            // Print the line below or single line
            name_clone.truncate(max_width.into());
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

impl Chart {
    fn new(metric_type: MetricType, exec_agg: Option<Arc<RwLock<ExecAgg>>>) -> Self {
        if let Some(exec_agg) = exec_agg {
            Self {
                exec_agg: exec_agg.try_read().unwrap().clone(), // Unwrap because app is initializing
                metric_type,
            }
        } else {
            Self {
                exec_agg: ExecAgg::default(),
                metric_type,
            }
        }
    }

    fn start_periodic_updates(chart: Arc<RwLock<Self>>, exec_agg: Arc<RwLock<ExecAgg>>) {
        tokio::spawn(async move {
            loop {
                let new_data = exec_agg.read().await.clone();

                debug!(
                    "Chart update: found {} data points",
                    new_data.exec_data.len()
                );

                {
                    let mut chart = chart.write().await;
                    chart.exec_agg = new_data;
                    trace!("Chart updated successfully");
                }

                tokio::time::sleep(Duration::from_secs(1)).await; // Update the data each second
            }
        });
    }
}

impl App {
    fn run(
        &mut self,
        terminal: &mut DefaultTerminal,
        chart1: &Arc<RwLock<Chart>>,
        chart2: &Arc<RwLock<Chart>>,
    ) -> std::io::Result<()> {
        let chart1_ref = chart1.clone();
        let chart2_ref = chart2.clone();

        while !self.exit {
            terminal.draw(|frame| self.draw(frame, &chart1_ref, &chart2_ref))?;
            self.handle_events()?;
            self.handle_pages(&chart1_ref, &chart2_ref);
        }

        Ok(())
    }

    fn handle_pages(&self, chart1: &Arc<RwLock<Chart>>, chart2: &Arc<RwLock<Chart>>) {
        // Get the current page configuration
        if let Some(page) = self.pages.get(self.current_page).cloned() {
            // Clone the Arc references to extend their lifetime
            let chart1 = chart1.clone();
            let chart2 = chart2.clone();

            // Update chart metrics based on the current page
            tokio::spawn(async move {
                match page {
                    Page::First((metric1, metric2)) => {
                        let mut c1 = chart1.write().await;
                        c1.metric_type = metric1.clone();

                        let mut c2 = chart2.write().await;
                        c2.metric_type = metric2.clone();
                    }
                    Page::Second((metric1, metric2)) => {
                        let mut c1 = chart1.write().await;
                        c1.metric_type = metric1.clone();

                        let mut c2 = chart2.write().await;
                        c2.metric_type = metric2.clone();
                    }
                }
            });
        }
    }

    fn draw(&self, frame: &mut Frame, chart1: &Arc<RwLock<Chart>>, chart2: &Arc<RwLock<Chart>>) {
        let layout = Layout::new(
            Direction::Vertical,
            vec![Constraint::Percentage(50), Constraint::Percentage(50)],
        )
        .split(frame.area());

        frame.render_widget(ChartWidget(chart1.clone()), layout[0]);
        frame.render_widget(ChartWidget(chart2.clone()), layout[1]);
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
            _ => {}
        }
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

    let chart1 = Arc::new(RwLock::new(Chart::new(MetricType::TotalExecTime, None)));
    let chart2 = Arc::new(RwLock::new(Chart::new(MetricType::TotalExecCount, None)));

    Chart::start_periodic_updates(chart1.clone(), exec_agg.clone());
    Chart::start_periodic_updates(chart2.clone(), exec_agg);

    let mut terminal = ratatui::init();
    let app_result = App::default().run(&mut terminal, &chart1, &chart2);
    ratatui::restore();
    app_result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_exit() {
        let mut app = App::default();
        app.handle_key_event(KeyCode::Char('q').into());

        assert!(app.exit);
    }
}
