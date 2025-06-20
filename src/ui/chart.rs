use ratatui::{
    buffer::Buffer,
    layout::{Constraint, Direction, Layout, Rect},
    style::Stylize,
    symbols::border,
    text::{Line, Text},
    widgets::{Block, Paragraph, Widget},
};
use std::fmt::Display;
use std::rc::Rc;

use tracing::{debug, info, trace, warn};

use crate::{ExecAgg, analytics::requests::ExecAggDiff};

use std::{collections::HashMap, sync::Arc};
use tokio::sync::RwLock;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) enum MetricType {
    #[default]
    TotalExecTime,
    TotalMemoryUsage,
    TotalExecCount,
    TimeAverage,
    DiffView(Box<MetricType>),
}

impl Display for MetricType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let val = match self {
            MetricType::TotalExecTime => "Total Execution Time (ms)",
            MetricType::TotalMemoryUsage => "Total Memory Usage (mb)",
            MetricType::TotalExecCount => "Total executions",
            MetricType::TimeAverage => "Average Time Per Execution (ms)",
            MetricType::DiffView(metric) => &format!("Captures difference - {}", metric.to_owned()),
        };

        write!(f, "{}", val)
    }
}

#[derive(Debug, Default, Clone)]
pub(super) struct Chart {
    pub(super) metric_type: MetricType,
    pub(super) exec_agg: Option<Arc<RwLock<ExecAgg>>>,
    pub(super) diff_data: Option<ExecAggDiff>,
}

impl Widget for &Chart {
    fn render(self, area: Rect, buf: &mut Buffer)
    where
        Self: Sized,
    {
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

        // Label for the x-axis
        Paragraph::new(Text::from(vec![Line::from(
            "Applications".to_string().yellow(),
        )]))
        .centered()
        .render(inner_layout[1], buf);

        let gap: u16 = 2; // gap between bars

        match &self.metric_type {
            MetricType::DiffView(metric_type) => {
                debug!(?metric_type, "Rendering diff view");
                self.clone()
                    .render_diff(inner_layout, gap, *metric_type.to_owned(), area, buf);
            }
            _ => {
                debug!(?self.metric_type, "Rendering view");
                self.render_metrics(inner_layout, gap, self.clone_agg_data(), area, buf)
            }
        }
    }
}

impl Chart {
    pub(super) fn new(metric_type: MetricType) -> Self {
        Self {
            metric_type,
            exec_agg: None,
            diff_data: None,
        }
    }

    pub(super) fn render_metrics(
        &self,
        inner_layout: Rc<[Rect]>,
        gap: u16,
        exec_agg_data: ExecAgg,
        area: Rect,
        buf: &mut Buffer,
    ) {
        let data = exec_agg_data.agg_data;

        // Early return if no data to display
        if data.is_empty() {
            info!("No data to render, skipping chart");
            return;
        }

        let data_len = data.len();
        debug!("Rendering chart with {} data points", data_len);

        let bars = if data_len == 0 {
            HashMap::new()
        } else {
            // Find the maximum execution time for scaling
            let max_val = data
                .values()
                .map(|d| match self.metric_type {
                    MetricType::TotalExecTime => d.total_exec_time,
                    MetricType::TotalMemoryUsage => d.total_memory_usage,
                    MetricType::TotalExecCount => d.total_execs as f32,
                    MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                    // Diff view is not handled here.
                    _ => 0.0,
                })
                .fold(0.0, f32::max);

            let min_val = data
                .values()
                .map(|d| match self.metric_type {
                    MetricType::TotalExecTime => d.total_exec_time,
                    MetricType::TotalMemoryUsage => d.total_memory_usage,
                    MetricType::TotalExecCount => d.total_execs as f32,
                    MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                    // Diff view is not handled here.
                    _ => 0.0,
                })
                .fold(0.0, f32::min);

            let base_y = if min_val < 0.0 {
                inner_layout[0].height.saturating_sub(3) / 2
            } else {
                inner_layout[0].height.saturating_sub(3) // 3 rows
            };

            // Calculate appropriate bar width to use full space
            let bar_width = inner_layout[0].width / data_len as u16;

            let mut map = HashMap::new();

            for (i, d) in data.values().enumerate() {
                let val = match self.metric_type {
                    MetricType::TotalExecTime => d.total_exec_time,
                    MetricType::TotalMemoryUsage => d.total_memory_usage,
                    MetricType::TotalExecCount => d.total_execs as f32,
                    MetricType::TimeAverage => d.total_exec_time / d.total_execs as f32,
                    // Diff view is not handled here.
                    _ => 0.0,
                };

                let height = self.resolve_height(val, max_val, min_val, &inner_layout);

                // X position with proper offset from inner_layout, gap allows centering the
                // bars properly
                let x = inner_layout[0].x + (i as u16 * bar_width + gap);

                trace!(%val, "Capturing");

                let rect = if val >= 0.0 {
                    // For positive values, ensure y coordinate is valid
                    let bar_y = inner_layout[0].y + base_y.saturating_sub(height);
                    // Ensure the rectangle stays within bounds
                    Rect::new(x, bar_y, bar_width.saturating_sub(gap), height.min(base_y))
                } else {
                    // For negative values
                    let adjusted_height = height.min(inner_layout[0].height.saturating_sub(base_y));
                    Rect::new(
                        x,
                        inner_layout[0].y + base_y,
                        bar_width.saturating_sub(gap),
                        adjusted_height,
                    )
                };

                map.insert((d.name.clone(), (val * 100.0).round() as i32), rect);
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
            for y_offset in 0..bar.height {
                // Ensure we don't go beyond buffer bounds
                if bar.y + y_offset >= area.y + area.height {
                    break; // Stop if we would exceed vertical bounds
                }

                // Calculate safe width for this line
                let safe_width = std::cmp::min(
                    bar.width as usize,
                    // Prevent potential buffer overflow by limiting width
                    if bar.x + bar.width <= area.x + area.width {
                        bar.width as usize
                    } else {
                        area.width.saturating_sub(bar.x - area.x) as usize
                    },
                );

                if safe_width > 0 {
                    let text = "█".repeat(safe_width);
                    let text = match self.metric_type {
                        MetricType::TotalExecCount | MetricType::TotalMemoryUsage => text.yellow(),
                        MetricType::TotalExecTime | MetricType::TimeAverage => text.blue(),
                        MetricType::DiffView(_) => text.red(),
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
            let value_rect = Rect::new(bar.x, bar.y.saturating_sub(2), bar.width, 1);
            Paragraph::new(
                Line::from(format!("{:.2}", values.1 as f32 / 100.0))
                    .yellow()
                    .centered(),
            )
            .render(value_rect, buf);

            Paragraph::new(bar_w.clone()).render(bar, buf);
        }
    }

    pub(super) fn render_diff(
        &mut self,
        inner_layout: Rc<[Rect]>,
        gap: u16,
        metric_type: MetricType,
        area: Rect,
        buf: &mut Buffer,
    ) {
        if let Some(diff_data) = &self.diff_data {
            let data = diff_data.exec_agg.clone();
            self.metric_type = metric_type; // Update to required metric
            self.render_metrics(inner_layout, gap, data, area, buf);
        } else {
            // Fallback, this should not be happen
            warn!("Diff data not found, fallback to agg_data");
            self.render_metrics(inner_layout, gap, self.clone_agg_data(), area, buf);
        }
    }

    fn resolve_height(
        &self,
        val: f32,
        max_val: f32,
        min_val: f32,
        inner_layout: &Rc<[Rect]>,
    ) -> u16 {
        // For negative values, ensure the bars have proportional heights
        if val < 0.0 {
            let abs_val = val.abs();
            let abs_min = min_val.abs();

            // Calculate the proportion based on the range of negative values
            let proportion = if abs_min < f32::EPSILON {
                0.0 // Avoid division by zero
            } else {
                abs_val / abs_min
            };

            // Calculate available height (half the space for negative values)
            let available_height = inner_layout[0].height as f32 / 2.0 * 0.7;
            let height_f32 = proportion * available_height;

            return (height_f32.round() as u16)
                .max(1)
                .min(inner_layout[0].height / 2);
        }

        // Positive values
        let range = max_val;

        // Ensure we don't divide by zero
        if range < f32::EPSILON {
            return 1; // Return minimum height if range is effectively zero
        }

        let available_height = inner_layout[0].height as f32 * 0.7;
        let height_f32 = (val / range) * available_height;
        let height = (height_f32.round() as u16).max(1);

        // Ensure height doesn't exceed available space
        height.min(inner_layout[0].height.saturating_sub(3))
    }

    fn clone_agg_data(&self) -> ExecAgg {
        // Use a blocking operation to safely wait for the data.
        // This approach ensures that the data is retrieved and waits for it
        // if it is not available, instead of using try_read in multiple places.
        // Also, the fallback with empty values is unexpected here. It is important
        // consider that the [`ExecAgg`] can be expensive if the data is large.
        // In this case, the data only stores aggregate data and "groups" it into a few fields.
        tokio::task::block_in_place(|| {
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
        })
    }
}
