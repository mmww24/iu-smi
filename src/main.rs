mod cli;
mod collector;
mod render;

use std::collections::VecDeque;
use std::io::{self, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use clap::Parser;
use cli::{Cli, Command, OutputFormat};
use collector::{collect_sample, Sample};
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;
use render::{render_app, SortMode, UiState};

const POLL_INTERVALS: [u64; 6] = [1, 5, 10, 15, 30, 60];

fn main() -> anyhow_like::Result<()> {
    let cli = Cli::parse();
    match cli.command.unwrap_or(Command::Local {
        interval: cli.interval,
        once: false,
    }) {
        Command::Local { interval, once } => {
            if once {
                let sample = collect_sample(normalize_interval(interval))?;
                print_snapshot(&sample, OutputFormat::Json, false)?;
            } else {
                run_tui(interval)?;
            }
        }
        Command::Snapshot {
            format,
            samples,
            interval,
            interval_ms,
            processes,
        } => run_snapshot(format, samples, interval, interval_ms, processes)?,
        Command::Api { port, interval } => run_api(port, normalize_interval(interval))?,
        Command::Doctor => run_doctor()?,
    }
    Ok(())
}

fn run_tui(interval_secs: u64) -> anyhow_like::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut state = UiState {
        sort_mode: SortMode::Default,
        filter_active_processes: false,
        show_help: false,
        selected_panel: 0,
        interval_secs: normalize_interval(interval_secs),
        history_cpu: VecDeque::with_capacity(120),
        history_gpu: VecDeque::with_capacity(120),
        history_npu: VecDeque::with_capacity(120),
        history_mem: VecDeque::with_capacity(120),
        last_error: None,
    };

    let result = run_tui_loop(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    result
}

fn run_tui_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    state: &mut UiState,
) -> anyhow_like::Result<()> {
    loop {
        let started = Instant::now();
        let sample = match collect_sample(state.interval_secs) {
            Ok(sample) => {
                state.last_error = None;
                sample
            }
            Err(err) => {
                state.last_error = Some(err.to_string());
                Sample::empty(state.interval_secs)
            }
        };
        push_history(state, &sample);
        terminal.draw(|frame| render_app(frame, state, &sample))?;

        let target = Duration::from_millis(150);
        while started.elapsed() < target {
            if event::poll(Duration::from_millis(20))? {
                if handle_event(state)? {
                    return Ok(());
                }
            }
        }

        while event::poll(Duration::from_millis(1))? {
            if handle_event(state)? {
                return Ok(());
            }
        }
    }
}

fn handle_event(state: &mut UiState) -> anyhow_like::Result<bool> {
    if let Event::Key(key) = event::read()? {
        if key.kind != KeyEventKind::Press {
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Esc if !state.show_help => return Ok(true),
            KeyCode::Esc => state.show_help = false,
            KeyCode::Char('1') | KeyCode::Char('h') | KeyCode::F(1) => {
                state.show_help = !state.show_help;
            }
            KeyCode::Char('d') => state.sort_mode = SortMode::Default,
            KeyCode::Char('u') => state.sort_mode = SortMode::Utilization,
            KeyCode::Char('g') => state.sort_mode = SortMode::GpuMemory,
            KeyCode::Char('p') => state.sort_mode = SortMode::Pid,
            KeyCode::Char('c') => state.sort_mode = SortMode::Cpu,
            KeyCode::Char('f') => state.filter_active_processes = !state.filter_active_processes,
            KeyCode::Char('+') | KeyCode::Char('=') | KeyCode::Right => {
                state.interval_secs = next_interval(state.interval_secs);
            }
            KeyCode::Char('-') | KeyCode::Left => {
                state.interval_secs = prev_interval(state.interval_secs);
            }
            KeyCode::Tab => state.selected_panel = (state.selected_panel + 1) % 2,
            _ => {}
        }
    }
    Ok(false)
}

fn normalize_interval(value: u64) -> u64 {
    POLL_INTERVALS
        .iter()
        .copied()
        .min_by_key(|candidate| candidate.abs_diff(value))
        .unwrap_or(1)
}

fn next_interval(value: u64) -> u64 {
    let current = normalize_interval(value);
    POLL_INTERVALS
        .iter()
        .copied()
        .find(|candidate| *candidate > current)
        .unwrap_or(current)
}

fn prev_interval(value: u64) -> u64 {
    let current = normalize_interval(value);
    POLL_INTERVALS
        .iter()
        .rev()
        .copied()
        .find(|candidate| *candidate < current)
        .unwrap_or(current)
}

fn push_history(state: &mut UiState, sample: &Sample) {
    push_one(&mut state.history_cpu, sample.cpu.utilization);
    push_one(&mut state.history_gpu, sample.gpu.utilization);
    push_one(
        &mut state.history_npu,
        sample.npu.utilization.unwrap_or(0.0),
    );
    push_one(&mut state.history_mem, sample.memory.utilization);
}

fn push_one(queue: &mut VecDeque<f64>, value: f64) {
    if queue.len() == 120 {
        queue.pop_front();
    }
    queue.push_back(value);
}

fn run_snapshot(
    format: OutputFormat,
    samples: u32,
    interval_secs: u64,
    interval_ms: Option<u32>,
    processes: bool,
) -> anyhow_like::Result<()> {
    let samples = samples.max(1);
    let (ms, fast) = match interval_ms {
        Some(ms) => (ms.max(1), ms < 1000),
        None => ((normalize_interval(interval_secs) * 1000) as u32, false),
    };
    if samples > 1 {
        collector::warm_caches();
    }
    if samples == 1 {
        let sample = collector::collect_sample_millis(ms, fast)?;
        print_snapshot(&sample, format, processes)?;
        return Ok(());
    }

    let mut out = Vec::with_capacity(samples as usize);
    for _ in 0..samples {
        out.push(collector::collect_sample_millis(ms, fast)?);
    }

    match format {
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&out)?);
        }
        OutputFormat::Csv => print_csv_many(&out, processes),
        OutputFormat::Prometheus => {
            for sample in out {
                print_prometheus(&sample, processes);
            }
        }
        OutputFormat::Nvidia => {
            for sample in out {
                print_nvidia_like(&sample);
            }
        }
    }
    Ok(())
}

fn print_snapshot(sample: &Sample, format: OutputFormat, processes: bool) -> anyhow_like::Result<()> {
    match format {
        OutputFormat::Json => println!("{}", serde_json::to_string_pretty(sample)?),
        OutputFormat::Csv => print_csv_many(std::slice::from_ref(sample), processes),
        OutputFormat::Prometheus => print_prometheus(sample, processes),
        OutputFormat::Nvidia => print_nvidia_like(sample),
    }
    Ok(())
}

fn gpu_engines_csv(sample: &Sample) -> String {
    sample
        .gpu
        .engines
        .iter()
        .map(|(engine, value)| format!("{engine}={value:.1}"))
        .collect::<Vec<_>>()
        .join(";")
}

fn print_csv_many(samples: &[Sample], processes: bool) {
    if !processes {
        println!("timestamp,hostname,cpu_pct,ram_used_gib,ram_total_gib,gpu_pct,npu_pct,gpu_shared_gib,gpu_dedicated_gib,openvino_devices");
        for sample in samples {
            println!(
                "{},{},{:.1},{:.3},{:.3},{:.1},{:.1},{:.3},{:.3},{}",
                sample.timestamp,
                sample.hostname,
                sample.cpu.utilization,
                sample.memory.used_bytes as f64 / 1_073_741_824.0,
                sample.memory.total_bytes as f64 / 1_073_741_824.0,
                sample.gpu.utilization,
                sample.npu.utilization.unwrap_or(0.0),
                sample.gpu.shared_memory_bytes as f64 / 1_073_741_824.0,
                sample.gpu.dedicated_memory_bytes as f64 / 1_073_741_824.0,
                sample.openvino.devices.join("|")
            );
        }
        return;
    }
    println!("timestamp,timestamp_ms,hostname,row_type,cpu_pct,ram_used_gib,ram_total_gib,gpu_pct,npu_pct,gpu_shared_gib,gpu_dedicated_gib,gpu_engines,openvino_devices,pid,process_name,unit,engine,process_utilization");
    for sample in samples {
        println!(
            "{},{},{},sample,{:.1},{:.3},{:.3},{:.1},{:.1},{:.3},{:.3},{},{},,,,",
            sample.timestamp,
            sample.timestamp_ms,
            sample.hostname,
            sample.cpu.utilization,
            sample.memory.used_bytes as f64 / 1_073_741_824.0,
            sample.memory.total_bytes as f64 / 1_073_741_824.0,
            sample.gpu.utilization,
            sample.npu.utilization.unwrap_or(0.0),
            sample.gpu.shared_memory_bytes as f64 / 1_073_741_824.0,
            sample.gpu.dedicated_memory_bytes as f64 / 1_073_741_824.0,
            gpu_engines_csv(sample),
            sample.openvino.devices.join("|")
        );
        for p in &sample.processes {
            println!(
                "{},{},{},process,,,,,,, ,,,,,{},{},{},{},{:.1}",
                sample.timestamp,
                sample.timestamp_ms,
                sample.hostname,
                p.pid,
                p.process_name,
                p.unit,
                p.engine,
                p.utilization
            );
        }
    }
}

fn print_prometheus(sample: &Sample, processes: bool) {
    println!(
        "iu_smi_cpu_utilization_percent {:.3}",
        sample.cpu.utilization
    );
    println!(
        "iu_smi_memory_utilization_percent {:.3}",
        sample.memory.utilization
    );
    println!("iu_smi_memory_used_bytes {}", sample.memory.used_bytes);
    println!("iu_smi_memory_total_bytes {}", sample.memory.total_bytes);
    println!(
        "iu_smi_gpu_utilization_percent {:.3}",
        sample.gpu.utilization
    );
    println!(
        "iu_smi_gpu_adapter_shared_bytes {}",
        sample.gpu.shared_memory_bytes
    );
    println!(
        "iu_smi_gpu_adapter_dedicated_bytes {}",
        sample.gpu.dedicated_memory_bytes
    );
    if let Some(npu) = sample.npu.utilization {
        println!("iu_smi_npu_utilization_percent {:.3}", npu);
    }
    for (engine, value) in &sample.gpu.engines {
        println!(
            "iu_smi_gpu_engine_utilization_percent{{engine=\"{engine}\"}} {:.3}",
            value
        );
    }
    if processes {
        for p in &sample.processes {
            let name = p.process_name.replace('"', "_");
            println!(
                "iu_smi_process_device_utilization_percent{{pid=\"{}\",process_name=\"{}\",unit=\"{}\",engine=\"{}\"}} {:.3}",
                p.pid, name, p.unit, p.engine, p.utilization
            );
        }
    }
}

fn print_nvidia_like(sample: &Sample) {
    println!("+-----------------------------------------------------------------------------------------+");
    println!(
        "| iu-smi {}                 Intel SoC Monitor                       |",
        sample.timestamp
    );
    println!(
        "| Host: {:<28} OpenVINO: {:<27} |",
        sample.hostname,
        sample.openvino.summary()
    );
    println!("+-----------------------------------------+------------------------+----------------------+");
    println!("| Device                                  | Utilization            | Memory               |");
    println!("+-----------------------------------------+------------------------+----------------------+");
    println!(
        "| CPU {:<35} | {:>6.1}%               | {:>5.1}/{:<5.1} GiB     |",
        sample.cpu.model,
        sample.cpu.utilization,
        sample.memory.used_bytes as f64 / 1_073_741_824.0,
        sample.memory.total_bytes as f64 / 1_073_741_824.0
    );
    println!(
        "| GPU Intel Xe2 integrated               | {:>6.1}%               | {:>5.1} GiB shared   |",
        sample.gpu.utilization,
        sample.gpu.shared_memory_bytes as f64 / 1_073_741_824.0
    );
    println!(
        "| NPU Intel AI Boost                     | {:>6}               | OpenVINO {:<9} |",
        sample
            .npu
            .utilization
            .map(|v| format!("{v:.1}%"))
            .unwrap_or_else(|| "N/A".to_string()),
        if sample.openvino.has_npu() {
            "ready"
        } else {
            "unknown"
        }
    );
    println!("+-----------------------------------------------------------------------------------------+");
}

fn run_api(port: u16, interval_secs: u64) -> anyhow_like::Result<()> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    eprintln!("iu-smi api listening on http://127.0.0.1:{port}/metrics");
    for stream in listener.incoming() {
        let mut stream = stream?;
        let sample = collect_sample(interval_secs)?;
        write_http_metrics(&mut stream, &sample)?;
    }
    Ok(())
}

fn write_http_metrics(stream: &mut TcpStream, sample: &Sample) -> anyhow_like::Result<()> {
    let mut body = Vec::new();
    writeln!(
        body,
        "iu_smi_cpu_utilization_percent {:.3}",
        sample.cpu.utilization
    )?;
    writeln!(
        body,
        "iu_smi_memory_utilization_percent {:.3}",
        sample.memory.utilization
    )?;
    writeln!(
        body,
        "iu_smi_gpu_utilization_percent {:.3}",
        sample.gpu.utilization
    )?;
    if let Some(npu) = sample.npu.utilization {
        writeln!(body, "iu_smi_npu_utilization_percent {:.3}", npu)?;
    }
    let header = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/plain; version=0.0.4\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(header.as_bytes())?;
    stream.write_all(&body)?;
    Ok(())
}

fn run_doctor() -> anyhow_like::Result<()> {
    let sample = collect_sample(1)?;
    println!("iu-smi doctor");
    println!("  platform: {}", std::env::consts::OS);
    println!("  hostname: {}", sample.hostname);
    println!("  cpu: {}", sample.cpu.model);
    println!("  openvino: {}", sample.openvino.summary());
    println!(
        "  pdh gpu counters: {}",
        if sample.gpu.available {
            "ok"
        } else {
            "missing"
        }
    );
    println!(
        "  pdh npu neural counter: {}",
        if sample.npu.utilization.is_some() {
            "ok"
        } else {
            "missing"
        }
    );
    for warning in sample.warnings {
        println!("  warning: {warning}");
    }
    Ok(())
}

pub mod anyhow_like {
    pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
}
