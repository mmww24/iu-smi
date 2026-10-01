use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::iter;
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use chrono::Local;
use regex::Regex;
use serde::Serialize;

pub fn epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Clone, Debug, Serialize)]
pub struct Sample {
    pub timestamp: String,
    pub timestamp_ms: u64,
    pub hostname: String,
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub gpu: GpuInfo,
    pub npu: NpuInfo,
    pub processes: Vec<ProcessInfo>,
    pub openvino: OpenVinoInfo,
    pub warnings: Vec<String>,
    pub interval_seconds: u64,
}

impl Sample {
    pub fn empty(interval_seconds: u64) -> Self {
        Self {
            timestamp: Local::now().format("%Y-%m-%d %H:%M:%S%.3f").to_string(),
            timestamp_ms: epoch_ms(),
            hostname: hostname(),
            cpu: CpuInfo::default(),
            memory: MemoryInfo::default(),
            gpu: GpuInfo::default(),
            npu: NpuInfo::default(),
            processes: Vec::new(),
            openvino: OpenVinoInfo::from_env(),
            warnings: Vec::new(),
            interval_seconds,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct CpuInfo {
    pub model: String,
    pub utilization: f64,
    pub frequency_mhz: Option<u32>,
    pub power_w: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
    pub utilization: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct GpuInfo {
    pub name: String,
    pub utilization: f64,
    pub shared_memory_bytes: u64,
    pub dedicated_memory_bytes: u64,
    pub engines: HashMap<String, f64>,
    pub available: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct NpuInfo {
    pub name: String,
    pub utilization: Option<f64>,
    pub engine_seen: bool,
    pub openvino_ready: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub process_name: String,
    pub unit: String,
    pub engine: String,
    pub utilization: f64,
    pub gpu_memory_bytes: u64,
    pub cpu_percent: f64,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct OpenVinoInfo {
    pub devices: Vec<String>,
    pub version: Option<String>,
    pub source: String,
}

impl OpenVinoInfo {
    pub fn from_env() -> Self {
        let devices = std::env::var("IU_SMI_OPENVINO_DEVICES")
            .unwrap_or_default()
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect::<Vec<_>>();
        Self {
            devices,
            version: std::env::var("IU_SMI_OPENVINO_VERSION").ok(),
            source: std::env::var("IU_SMI_OPENVINO_SOURCE")
                .unwrap_or_else(|_| "python-wrapper".to_string()),
        }
    }

    pub fn has_npu(&self) -> bool {
        self.devices
            .iter()
            .any(|d| d.to_ascii_uppercase().contains("NPU"))
    }

    pub fn summary(&self) -> String {
        if self.devices.is_empty() {
            "not detected".to_string()
        } else {
            self.devices.join(",")
        }
    }
}

pub fn collect_sample(interval_seconds: u64) -> crate::anyhow_like::Result<Sample> {
    collect_sample_millis((interval_seconds.max(1) * 1000) as u32, false)
}

/// Pre-fetch the cached process-name map and CPU model so that every sample in a
/// time-series run skips the first-call PowerShell spawn and keeps its interval.
pub fn warm_caches() {
    let _ = process_name_map();
    let _ = cpu_model();
}

pub fn collect_sample_millis(
    interval_ms: u32,
    fast: bool,
) -> crate::anyhow_like::Result<Sample> {
    collect_platform_sample(interval_ms.max(1), fast)
}

#[cfg(windows)]
fn collect_platform_sample(interval_ms: u32, use_ps_fallback: bool) -> crate::anyhow_like::Result<Sample> {
    let mut warnings = Vec::new();
    let cpu_start = native::read_cpu_times();

    let mut query = 0isize;
    let mut gpu_util = 0isize;
    let mut dedicated_mem = 0isize;
    let mut shared_mem = 0isize;

    unsafe {
        let status = native::PdhOpenQueryW(std::ptr::null(), 0, &mut query);
        if status != native::ERROR_SUCCESS {
            warnings.push(format!("PdhOpenQuery failed: 0x{status:08x}"));
        } else {
            add_counter(
                query,
                "\\GPU Engine(*)\\Utilization Percentage",
                &mut gpu_util,
                &mut warnings,
            );
            add_counter(
                query,
                "\\GPU Adapter Memory(*)\\Dedicated Usage",
                &mut dedicated_mem,
                &mut warnings,
            );
            add_counter(
                query,
                "\\GPU Adapter Memory(*)\\Shared Usage",
                &mut shared_mem,
                &mut warnings,
            );
            native::PdhCollectQueryData(query);
        }
    }

    thread::sleep(Duration::from_millis(interval_ms as u64));

    let cpu_end = native::read_cpu_times();
    let memory = native::read_memory();
    let mut sample = Sample::empty(((interval_ms / 1000).max(1)) as u64);
    sample.cpu = CpuInfo {
        model: cpu_model(),
        utilization: cpu_percent(cpu_start, cpu_end),
        frequency_mhz: None,
        power_w: None,
    };
    sample.memory = memory;
    sample.openvino = OpenVinoInfo::from_env();
    sample.warnings = warnings;

    if query != 0 {
        unsafe {
            native::PdhCollectQueryData(query);
        }
        let process_names = process_name_map();
        apply_gpu(&mut sample, gpu_util, &process_names);
        if use_ps_fallback {
            merge_powershell_gpu_counters(&mut sample, &process_names);
        }
        sample.gpu.dedicated_memory_bytes = sum_counter_bytes(dedicated_mem);
        sample.gpu.shared_memory_bytes = sum_counter_bytes(shared_mem);
        unsafe {
            native::PdhCloseQuery(query);
        }
    }

    sample.npu.openvino_ready = sample.openvino.has_npu();
    Ok(sample)
}

#[cfg(not(windows))]
fn collect_platform_sample(interval_ms: u32, _use_ps_fallback: bool) -> crate::anyhow_like::Result<Sample> {
    thread::sleep(Duration::from_millis(interval_ms as u64));
    let mut sample = Sample::empty(((interval_ms / 1000).max(1)) as u64);
    sample.cpu.model = std::env::consts::ARCH.to_string();
    sample.openvino = OpenVinoInfo::from_env();
    sample
        .warnings
        .push("native utilization counters are implemented for Windows PDH".to_string());
    Ok(sample)
}

#[cfg(windows)]
fn add_counter(query: isize, path: &str, counter: &mut isize, warnings: &mut Vec<String>) {
    let wide = wide(path);
    let status = unsafe { native::PdhAddEnglishCounterW(query, wide.as_ptr(), 0, counter) };
    if status != native::ERROR_SUCCESS {
        warnings.push(format!("{path} unavailable: 0x{status:08x}"));
    }
}

#[cfg(windows)]
fn apply_gpu(sample: &mut Sample, gpu_util: isize, process_names: &HashMap<u32, String>) {
    if gpu_util == 0 {
        return;
    }

    let pid_re = Regex::new(r"(?i)pid_(\d+)").expect("pid regex");
    let engine_re = Regex::new(r"(?i)engtype_([^_)]+)").expect("engine regex");
    let luid_re = Regex::new(r"(?i)(luid_0x[0-9a-f]+_0x[0-9a-f]+_phys_\d+)").expect("luid regex");
    let values = native::read_formatted_double_array(gpu_util);
    sample.gpu.available = !values.is_empty();
    sample.gpu.name = "Intel Xe2 integrated GPU".to_string();
    sample.npu.name = "Intel AI Boost NPU".to_string();

    let npu_luids = classify_npu_luids(&values, &luid_re, &engine_re);
    if std::env::var("IU_SMI_DEBUG_LUIDS").is_ok() {
        sample
            .warnings
            .push(format!("npu_luids={}", join_set(&npu_luids)));
    }
    let mut process_index: HashMap<String, usize> = HashMap::new();
    for value in &values {
        let pid = pid_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if pid == 0 {
            continue;
        }

        let engine = engine_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_else(|| "other".to_string());
        let percent = value.value.clamp(0.0, 100.0);
        let luid = luid_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase());
        if std::env::var("IU_SMI_DEBUG_LUIDS").is_ok() && percent > 10.0 {
            sample.warnings.push(format!(
                "active_instance={} luid={:?} engine={} npu_luids={}",
                value.name,
                luid,
                engine,
                join_set(&npu_luids)
            ));
        }
        let is_npu = luid
            .as_ref()
            .map(|luid| npu_luids.contains(luid))
            .unwrap_or_else(|| npu_luids.is_empty() && engine == "neural");
        let unit = if is_npu { "NPU" } else { "GPU" };

        if unit == "NPU" {
            sample.npu.engine_seen = true;
            let next = sample.npu.utilization.unwrap_or(0.0) + percent;
            sample.npu.utilization = Some(next.clamp(0.0, 100.0));
        } else {
            *sample.gpu.engines.entry(engine.clone()).or_default() += percent;
        }

        let key = format!("{pid}|{unit}|{engine}");
        let index = if let Some(index) = process_index.get(&key) {
            *index
        } else {
            let index = sample.processes.len();
            process_index.insert(key, index);
            sample.processes.push(ProcessInfo {
                pid,
                process_name: process_names
                    .get(&pid)
                    .cloned()
                    .unwrap_or_else(|| format!("pid_{pid}")),
                unit: unit.to_string(),
                engine: engine.clone(),
                utilization: 0.0,
                gpu_memory_bytes: 0,
                cpu_percent: 0.0,
            });
            index
        };
        sample.processes[index].utilization += percent;
    }

    sample.gpu.utilization = sample
        .gpu
        .engines
        .values()
        .copied()
        .fold(0.0f64, f64::max)
        .clamp(0.0, 100.0);
    sample.processes.sort_by(|a, b| {
        b.utilization
            .partial_cmp(&a.utilization)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[cfg(windows)]
fn merge_powershell_gpu_counters(sample: &mut Sample, process_names: &HashMap<u32, String>) {
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Counter '\\GPU Engine(*)\\Utilization Percentage' -SampleInterval 1 -MaxSamples 2 | ForEach-Object { $_.CounterSamples } | ForEach-Object { \"$($_.CookedValue)|$($_.InstanceName)\" }",
        ])
        .output();
    let Ok(output) = output else {
        return;
    };
    if !output.status.success() {
        return;
    }

    let pid_re = Regex::new(r"(?i)pid_(\d+)").expect("pid regex");
    let engine_re = Regex::new(r"(?i)engtype_([^_)]+)").expect("engine regex");
    let luid_re = Regex::new(r"(?i)(luid_0x[0-9a-f]+_0x[0-9a-f]+_phys_\d+)").expect("luid regex");
    let mut raw_by_name: HashMap<String, f64> = HashMap::new();
    let text = String::from_utf8_lossy(&output.stdout);
    for line in text.lines() {
        let Some((value, instance)) = line.split_once('|') else {
            continue;
        };
        let Ok(value) = value.trim().parse::<f64>() else {
            continue;
        };
        let instance = instance.trim().to_string();
        let current = raw_by_name.entry(instance).or_default();
        if value > *current {
            *current = value;
        }
    }
    let raw_values = raw_by_name
        .into_iter()
        .map(|(name, value)| native::PdhValue { name, value })
        .collect::<Vec<_>>();
    if raw_values.is_empty() {
        if std::env::var("IU_SMI_DEBUG_LUIDS").is_ok() {
            sample
                .warnings
                .push("powershell_fallback_empty".to_string());
        }
        return;
    }
    if std::env::var("IU_SMI_DEBUG_LUIDS").is_ok() {
        let max = raw_values
            .iter()
            .max_by(|a, b| {
                a.value
                    .partial_cmp(&b.value)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .map(|v| format!("{}={:.2}", v.name, v.value))
            .unwrap_or_else(|| "none".to_string());
        sample.warnings.push(format!(
            "powershell_fallback_count={} max={max}",
            raw_values.len()
        ));
    }

    let npu_luids = classify_npu_luids(&raw_values, &luid_re, &engine_re);
    let mut gpu_by_engine: HashMap<String, f64> = HashMap::new();
    let mut npu_util = 0.0f64;
    let mut merged: HashMap<String, ProcessInfo> = HashMap::new();

    for value in raw_values {
        let pid = pid_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .and_then(|m| m.as_str().parse::<u32>().ok())
            .unwrap_or(0);
        if pid == 0 {
            continue;
        }
        let engine = engine_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_else(|| "other".to_string());
        let luid = luid_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase());
        let is_npu = luid
            .as_ref()
            .map(|luid| npu_luids.contains(luid))
            .unwrap_or(false);
        let unit = if is_npu { "NPU" } else { "GPU" };
        let percent = value.value.clamp(0.0, 100.0);

        if is_npu {
            sample.npu.engine_seen = true;
            npu_util += percent;
        } else {
            *gpu_by_engine.entry(engine.clone()).or_default() += percent;
        }

        let key = format!("{pid}|{unit}|{engine}");
        let entry = merged.entry(key).or_insert_with(|| ProcessInfo {
            pid,
            process_name: process_names
                .get(&pid)
                .cloned()
                .unwrap_or_else(|| format!("pid_{pid}")),
            unit: unit.to_string(),
            engine: engine.clone(),
            utilization: 0.0,
            gpu_memory_bytes: 0,
            cpu_percent: 0.0,
        });
        entry.utilization += percent;
    }

    let fallback_gpu_util = gpu_by_engine
        .values()
        .copied()
        .fold(0.0f64, f64::max)
        .clamp(0.0, 100.0);
    if fallback_gpu_util > sample.gpu.utilization {
        sample.gpu.utilization = fallback_gpu_util;
        sample.gpu.engines = gpu_by_engine;
    }
    if npu_util > sample.npu.utilization.unwrap_or(0.0) {
        sample.npu.utilization = Some(npu_util.clamp(0.0, 100.0));
    }

    let mut by_key: HashMap<String, ProcessInfo> = sample
        .processes
        .drain(..)
        .map(|p| (format!("{}|{}|{}", p.pid, p.unit, p.engine), p))
        .collect();
    for (key, process) in merged {
        match by_key.get_mut(&key) {
            Some(existing) if process.utilization > existing.utilization => {
                existing.utilization = process.utilization;
                existing.process_name = process.process_name;
            }
            None => {
                by_key.insert(key, process);
            }
            _ => {}
        }
    }
    sample.processes = by_key.into_values().collect();
    sample.processes.sort_by(|a, b| {
        b.utilization
            .partial_cmp(&a.utilization)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
}

#[cfg(windows)]
fn join_set(values: &HashSet<String>) -> String {
    let mut values = values.iter().cloned().collect::<Vec<_>>();
    values.sort();
    values.join("|")
}

#[cfg(windows)]
fn classify_npu_luids(
    values: &[native::PdhValue],
    luid_re: &Regex,
    engine_re: &Regex,
) -> HashSet<String> {
    let mut engines_by_luid: HashMap<String, HashSet<String>> = HashMap::new();
    for value in values {
        let Some(luid) = luid_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase())
        else {
            continue;
        };
        let engine = engine_re
            .captures(&value.name)
            .and_then(|c| c.get(1))
            .map(|m| m.as_str().to_ascii_lowercase())
            .unwrap_or_else(|| "other".to_string());
        engines_by_luid.entry(luid).or_default().insert(engine);
    }

    engines_by_luid
        .into_iter()
        .filter_map(|(luid, engines)| {
            let has_graphics_or_media = engines.iter().any(|engine| {
                matches!(
                    engine.as_str(),
                    "3d" | "copy"
                        | "videodecode"
                        | "videoencode"
                        | "videoprocessing"
                        | "neural"
                        | "gsc"
                )
            });
            if engines.contains("compute") && !has_graphics_or_media {
                Some(luid)
            } else {
                None
            }
        })
        .collect()
}

#[cfg(windows)]
fn sum_counter_bytes(counter: isize) -> u64 {
    if counter == 0 {
        return 0;
    }
    native::read_formatted_double_array(counter)
        .into_iter()
        .filter(|v| v.value.is_finite() && v.value > 0.0)
        .map(|v| v.value as u64)
        .sum()
}

#[cfg(windows)]
fn cpu_percent(start: native::CpuTimes, end: native::CpuTimes) -> f64 {
    let idle = end.idle.saturating_sub(start.idle);
    let kernel = end.kernel.saturating_sub(start.kernel);
    let user = end.user.saturating_sub(start.user);
    let total = kernel + user;
    if total == 0 {
        0.0
    } else {
        ((total - idle) as f64 * 100.0 / total as f64).clamp(0.0, 100.0)
    }
}

static CPU_MODEL: OnceLock<String> = OnceLock::new();

fn cpu_model() -> String {
    CPU_MODEL.get_or_init(detect_cpu_model).clone()
}

fn detect_cpu_model() -> String {
    let output = Command::new("reg")
        .args([
            "query",
            "HKLM\\HARDWARE\\DESCRIPTION\\System\\CentralProcessor\\0",
            "/v",
            "ProcessorNameString",
        ])
        .output();
    if let Ok(output) = output {
        let text = String::from_utf8_lossy(&output.stdout);
        if let Some(line) = text
            .lines()
            .find(|line| line.contains("ProcessorNameString"))
        {
            let model = line
                .split("REG_SZ")
                .nth(1)
                .map(str::trim)
                .unwrap_or_default();
            if !model.is_empty() {
                return model.to_string();
            }
        }
    }
    "Intel Core Ultra SoC".to_string()
}

const PROCESS_NAME_TTL_MS: u64 = 30_000;

struct ProcessNameCache {
    fetched_at_ms: u64,
    map: HashMap<u32, String>,
}

static PROCESS_NAME_CACHE: OnceLock<Mutex<ProcessNameCache>> = OnceLock::new();

fn process_name_map() -> HashMap<u32, String> {
    let cache = PROCESS_NAME_CACHE
        .get_or_init(|| Mutex::new(ProcessNameCache { fetched_at_ms: 0, map: HashMap::new() }));
    let mut guard = cache.lock().unwrap();
    let now = epoch_ms();
    if !guard.map.is_empty() && now.saturating_sub(guard.fetched_at_ms) < PROCESS_NAME_TTL_MS {
        return guard.map.clone();
    }
    let mut map = HashMap::new();
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-Process | ForEach-Object { \"$($_.Id)|$($_.ProcessName)\" }",
        ])
        .output();
    if let Ok(output) = output {
        let text = String::from_utf8_lossy(&output.stdout);
        for line in text.lines() {
            let Some((pid, name)) = line.split_once('|') else {
                continue;
            };
            if let Ok(pid) = pid.trim().parse::<u32>() {
                let name = name.trim();
                if !name.is_empty() {
                    map.insert(pid, name.to_string());
                }
            }
        }
    }
    if !map.is_empty() {
        guard.map = map.clone();
        guard.fetched_at_ms = now;
    }
    map
}

fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "localhost".to_string())
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    OsStr::new(value)
        .encode_wide()
        .chain(iter::once(0))
        .collect()
}

#[cfg(windows)]
mod native {
    use super::MemoryInfo;

    pub const ERROR_SUCCESS: u32 = 0;
    const PDH_MORE_DATA: u32 = 0x8000_07D2;
    const PDH_FMT_DOUBLE: u32 = 0x0000_0200;

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct FileTime {
        pub dw_low_date_time: u32,
        pub dw_high_date_time: u32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    pub struct MemoryStatusEx {
        pub dw_length: u32,
        pub dw_memory_load: u32,
        pub ull_total_phys: u64,
        pub ull_avail_phys: u64,
        pub ull_total_page_file: u64,
        pub ull_avail_page_file: u64,
        pub ull_total_virtual: u64,
        pub ull_avail_virtual: u64,
        pub ull_avail_extended_virtual: u64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PdhFmtCounterValueDouble {
        c_status: u32,
        double_value: f64,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PdhFmtCounterValueItemDouble {
        sz_name: *const u16,
        fmt_value: PdhFmtCounterValueDouble,
    }

    #[derive(Clone, Copy, Default)]
    pub struct CpuTimes {
        pub idle: u64,
        pub kernel: u64,
        pub user: u64,
    }

    pub struct PdhValue {
        pub name: String,
        pub value: f64,
    }

    #[link(name = "pdh")]
    extern "system" {
        pub fn PdhOpenQueryW(data_source: *const u16, user_data: usize, query: *mut isize) -> u32;
        pub fn PdhAddEnglishCounterW(
            query: isize,
            full_counter_path: *const u16,
            user_data: usize,
            counter: *mut isize,
        ) -> u32;
        pub fn PdhCollectQueryData(query: isize) -> u32;
        pub fn PdhCloseQuery(query: isize) -> u32;
        fn PdhGetFormattedCounterArrayW(
            counter: isize,
            format: u32,
            buffer_size: *mut u32,
            item_count: *mut u32,
            item_buffer: *mut PdhFmtCounterValueItemDouble,
        ) -> u32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn GetSystemTimes(
            idle_time: *mut FileTime,
            kernel_time: *mut FileTime,
            user_time: *mut FileTime,
        ) -> i32;
        fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
    }

    pub fn read_cpu_times() -> CpuTimes {
        let mut idle = FileTime::default();
        let mut kernel = FileTime::default();
        let mut user = FileTime::default();
        let ok = unsafe { GetSystemTimes(&mut idle, &mut kernel, &mut user) };
        if ok == 0 {
            return CpuTimes::default();
        }
        CpuTimes {
            idle: to_u64(idle),
            kernel: to_u64(kernel),
            user: to_u64(user),
        }
    }

    pub fn read_memory() -> MemoryInfo {
        let mut mem = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            ..Default::default()
        };
        let ok = unsafe { GlobalMemoryStatusEx(&mut mem) };
        if ok == 0 {
            return MemoryInfo::default();
        }
        MemoryInfo {
            total_bytes: mem.ull_total_phys,
            used_bytes: mem.ull_total_phys.saturating_sub(mem.ull_avail_phys),
            utilization: mem.dw_memory_load as f64,
        }
    }

    pub fn read_formatted_double_array(counter: isize) -> Vec<PdhValue> {
        let mut buffer_size = 0u32;
        let mut item_count = 0u32;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                std::ptr::null_mut(),
            )
        };
        if status != PDH_MORE_DATA || buffer_size == 0 || item_count == 0 {
            return Vec::new();
        }

        let mut buffer = vec![0u8; buffer_size as usize];
        let item_ptr = buffer.as_mut_ptr() as *mut PdhFmtCounterValueItemDouble;
        let status = unsafe {
            PdhGetFormattedCounterArrayW(
                counter,
                PDH_FMT_DOUBLE,
                &mut buffer_size,
                &mut item_count,
                item_ptr,
            )
        };
        if status != ERROR_SUCCESS {
            return Vec::new();
        }

        let items = unsafe { std::slice::from_raw_parts(item_ptr, item_count as usize) };
        items
            .iter()
            .filter(|item| item.fmt_value.c_status == ERROR_SUCCESS)
            .map(|item| PdhValue {
                name: wide_ptr_to_string(item.sz_name),
                value: item.fmt_value.double_value,
            })
            .collect()
    }

    fn to_u64(time: FileTime) -> u64 {
        ((time.dw_high_date_time as u64) << 32) | time.dw_low_date_time as u64
    }

    fn wide_ptr_to_string(ptr: *const u16) -> String {
        if ptr.is_null() {
            return String::new();
        }
        let mut len = 0usize;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
            String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
        }
    }
}
