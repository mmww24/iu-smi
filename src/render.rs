use std::collections::VecDeque;

use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Clear, Paragraph, Row, Table, Wrap};
use ratatui::Frame;

use crate::collector::{ProcessInfo, Sample};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortMode {
    Default,
    Utilization,
    GpuMemory,
    Pid,
    Cpu,
}

pub struct UiState {
    pub sort_mode: SortMode,
    pub filter_active_processes: bool,
    pub show_help: bool,
    pub selected_panel: usize,
    pub interval_secs: u64,
    pub history_cpu: VecDeque<f64>,
    pub history_gpu: VecDeque<f64>,
    pub history_npu: VecDeque<f64>,
    pub history_mem: VecDeque<f64>,
    pub last_error: Option<String>,
}

pub fn render_app(frame: &mut Frame<'_>, state: &UiState, sample: &Sample) {
    let area = frame.area();
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Min(8),
            Constraint::Length(1),
        ])
        .split(area);

    render_header(frame, root[0], sample);
    render_overview(frame, root[1], state, sample);
    render_devices(frame, root[2], sample);
    render_processes(frame, root[3], state, sample);
    render_footer(frame, root[4], state, sample);

    if state.show_help {
        render_help(frame, centered_rect(64, 52, area));
    }
}

fn render_header(frame: &mut Frame<'_>, area: Rect, sample: &Sample) {
    let devices = if sample.openvino.devices.is_empty() {
        "OpenVINO devices: not detected".to_string()
    } else {
        format!("OpenVINO devices: {}", sample.openvino.devices.join(", "))
    };
    let title = vec![
        Line::from(vec![
            Span::styled(
                "iu-smi",
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw("  Intel Core Ultra SoC Monitor  "),
            Span::styled(
                sample.timestamp.as_str(),
                Style::default().fg(Color::DarkGray),
            ),
        ]),
        Line::from(vec![
            Span::styled(sample.hostname.as_str(), Style::default().fg(Color::Green)),
            Span::raw("  "),
            Span::raw(sample.cpu.model.as_str()),
            Span::raw("  "),
            Span::styled(devices, Style::default().fg(Color::Yellow)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(title).block(Block::default().borders(Borders::BOTTOM)),
        area,
    );
}

fn render_overview(frame: &mut Frame<'_>, area: Rect, state: &UiState, sample: &Sample) {
    let _ = state;
    let inner_width = area.width.saturating_sub(2) as usize;
    let bar_width = inner_width.saturating_sub(14).max(8);
    let rows = vec![
        metric_line("CPU", sample.cpu.utilization, bar_width),
        metric_line("GPU", sample.gpu.utilization, bar_width),
        metric_line("NPU", sample.npu.utilization.unwrap_or(0.0), bar_width),
        metric_line("Mem", sample.memory.utilization, bar_width),
    ];
    frame.render_widget(
        Paragraph::new(rows).block(Block::default().title(" Overview ").borders(Borders::ALL)),
        area,
    );
}

fn metric_line(label: &str, value: f64, width: usize) -> Line<'static> {
    let value = value.clamp(0.0, 100.0);
    let filled = ((value / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    let color = usage_color(value);
    Line::from(vec![
        Span::styled(
            format!("{label:<3} "),
            Style::default()
                .fg(Color::White)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw("["),
        Span::styled("=".repeat(filled), Style::default().fg(color)),
        Span::styled("-".repeat(empty), Style::default().fg(Color::DarkGray)),
        Span::raw(format!(" {value:>5.1}%]")),
    ])
}

fn render_devices(frame: &mut Frame<'_>, area: Rect, sample: &Sample) {
    let rows = vec![
        Row::new(vec![
            Cell::from("CPU"),
            Cell::from(sample.cpu.model.clone()),
            Cell::from(format!("{:.1}%", sample.cpu.utilization)),
            Cell::from(format!(
                "{:.1}/{:.1} GiB",
                sample.memory.used_bytes as f64 / 1_073_741_824.0,
                sample.memory.total_bytes as f64 / 1_073_741_824.0
            )),
            Cell::from(
                sample
                    .cpu
                    .frequency_mhz
                    .map(|v| format!("{v} MHz"))
                    .unwrap_or_else(|| "N/A".into()),
            ),
            Cell::from("Win32"),
        ]),
        Row::new(vec![
            Cell::from("GPU"),
            Cell::from(if sample.gpu.name.is_empty() {
                "Intel Xe2 integrated GPU"
            } else {
                &sample.gpu.name
            }),
            Cell::from(format!("{:.1}%", sample.gpu.utilization)),
            Cell::from(format!(
                "shared {:.1} GiB / dedicated {:.1} GiB",
                sample.gpu.shared_memory_bytes as f64 / 1_073_741_824.0,
                sample.gpu.dedicated_memory_bytes as f64 / 1_073_741_824.0
            )),
            Cell::from("N/A"),
            Cell::from("PDH GPU Engine"),
        ]),
        Row::new(vec![
            Cell::from("NPU"),
            Cell::from(if sample.npu.name.is_empty() {
                "Intel AI Boost NPU"
            } else {
                &sample.npu.name
            }),
            Cell::from(
                sample
                    .npu
                    .utilization
                    .map(|v| format!("{v:.1}%"))
                    .unwrap_or_else(|| "N/A".into()),
            ),
            Cell::from("OpenVINO runtime"),
            Cell::from("N/A"),
            Cell::from(if sample.openvino.has_npu() {
                "OpenVINO NPU"
            } else {
                "engtype_neural"
            }),
        ]),
    ];
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Percentage(28),
            Constraint::Length(10),
            Constraint::Percentage(28),
            Constraint::Length(12),
            Constraint::Percentage(20),
        ],
    )
    .header(
        Row::new(["TYPE", "NAME", "UTIL", "MEMORY", "CLOCK", "SOURCE"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(Block::default().title(" Devices ").borders(Borders::ALL))
    .row_highlight_style(Style::default().bg(Color::DarkGray));
    frame.render_widget(table, area);
}

fn render_processes(frame: &mut Frame<'_>, area: Rect, state: &UiState, sample: &Sample) {
    let mut processes = sample.processes.clone();
    if state.filter_active_processes {
        processes.retain(|p| p.utilization >= 0.05);
    }
    sort_processes(&mut processes, state.sort_mode);

    let rows = processes
        .iter()
        .take(area.height.saturating_sub(4) as usize)
        .map(|p| {
            Row::new(vec![
                Cell::from(p.pid.to_string()),
                Cell::from(p.process_name.clone()),
                Cell::from(p.unit.clone()),
                Cell::from(p.engine.clone()),
                Cell::from(format!("{:.1}", p.utilization)),
                Cell::from(format!("{:.1}", p.cpu_percent)),
                Cell::from(format_memory_short(p.gpu_memory_bytes)),
            ])
            .style(Style::default().fg(process_color(p)))
        });

    let title = format!(
        " Processes  sort={:?}  filter={} ",
        state.sort_mode,
        if state.filter_active_processes {
            "active"
        } else {
            "all"
        }
    );
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Percentage(28),
            Constraint::Length(6),
            Constraint::Percentage(18),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(
        Row::new(["PID", "PROCESS", "UNIT", "ENGINE", "GPU%", "CPU%", "VRAM"]).style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        ),
    )
    .block(
        Block::default()
            .title(title)
            .borders(Borders::ALL)
            .border_style(if state.selected_panel == 1 {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default()
            }),
    );
    frame.render_widget(table, area);
}

fn render_footer(frame: &mut Frame<'_>, area: Rect, state: &UiState, sample: &Sample) {
    let mut text = format!(
        "q quit | +/- interval {}s | h/1 help | d default | u util | g gpu-mem | p pid | c cpu | f filter | Tab panel",
        state.interval_secs
    );
    if let Some(error) = &state.last_error {
        text.push_str(" | error: ");
        text.push_str(error);
    } else if !sample.warnings.is_empty() {
        text.push_str(" | warning: ");
        text.push_str(&sample.warnings[0]);
    }
    frame.render_widget(
        Paragraph::new(text).style(Style::default().fg(Color::DarkGray)),
        area,
    );
}

fn render_help(frame: &mut Frame<'_>, area: Rect) {
    let text = vec![
        Line::from(Span::styled("iu-smi help", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))),
        Line::from(""),
        Line::from("q / Esc       quit"),
        Line::from("h / 1 / F1    toggle help"),
        Line::from("d             default process order"),
        Line::from("u             sort by utilization"),
        Line::from("g             sort by GPU memory"),
        Line::from("p             sort by PID"),
        Line::from("c             sort by CPU"),
        Line::from("+ / Right     increase polling interval"),
        Line::from("- / Left      decrease polling interval"),
        Line::from("f             show active GPU/NPU engines only"),
        Line::from("Tab           switch panel focus"),
        Line::from(""),
        Line::from("Metrics source: Windows PDH GPU Engine counters plus OpenVINO device discovery from the Python wrapper."),
    ];
    frame.render_widget(Clear, area);
    frame.render_widget(
        Paragraph::new(text)
            .block(Block::default().title(" Help ").borders(Borders::ALL))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true }),
        area,
    );
}

fn sort_processes(processes: &mut [ProcessInfo], mode: SortMode) {
    match mode {
        SortMode::Default | SortMode::Utilization => processes.sort_by(|a, b| {
            b.utilization
                .partial_cmp(&a.utilization)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        SortMode::GpuMemory => processes.sort_by_key(|p| std::cmp::Reverse(p.gpu_memory_bytes)),
        SortMode::Pid => processes.sort_by_key(|p| p.pid),
        SortMode::Cpu => processes.sort_by(|a, b| {
            b.cpu_percent
                .partial_cmp(&a.cpu_percent)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
}

fn usage_color(value: f64) -> Color {
    if value >= 80.0 {
        Color::Red
    } else if value >= 60.0 {
        Color::Yellow
    } else {
        Color::Green
    }
}

fn process_color(process: &ProcessInfo) -> Color {
    if process.utilization >= 80.0 {
        Color::Red
    } else if process.utilization >= 60.0 {
        Color::Yellow
    } else if process.unit == "NPU" {
        Color::Magenta
    } else if process.utilization > 0.0 {
        Color::Green
    } else {
        Color::DarkGray
    }
}

fn format_memory_short(bytes: u64) -> String {
    if bytes == 0 {
        return "-".to_string();
    }
    let gb = bytes as f64 / 1_073_741_824.0;
    if gb >= 1.0 {
        format!("{gb:.1}G")
    } else {
        format!("{:.0}M", bytes as f64 / 1_048_576.0)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
}
