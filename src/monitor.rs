use std::ffi::CString;
use std::process::Stdio;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdout, Command};
use tokio::sync::watch;
use tracing::{debug, error, info, warn};

use crate::types::
    {AtomicHealth, BatteryStatus, CoreData, CpuSnap, DevicePaths, MemoryInfo, MonitorHealth,
    ServiceStatus, StaticDeviceInfo, StorageInfo, SystemStats};

const MAX_RISH_RETRIES: u32 = 5;
const RISH_RETRY_DELAY: Duration = Duration::from_secs(2);
const STORAGE_TICK_INTERVAL: u64 = 60;

/// Batch shell command sent to `rish` every tick.
const RISH_CMD: &[u8] = b"echo UPTIME $(cat /proc/uptime); \
                           cat /proc/stat; \
                           dumpsys battery | grep -E 'level|status|temp'; \
                           echo DISPLAY_DATA; \
                           dumpsys display | grep -oE 'mBrightness=[0-9.]+|mActiveRenderFrameRate=[0-9.]+'; \
                           echo DISPLAY_END; \
                           echo 'END_OF_BATCH'\n";

/// Type alias for the async line reader over the rish stdout pipe.
type RishLines = Lines<BufReader<ChildStdout>>;

/// Parsed output from a single rish batch tick.
struct RishTick {
    uptime_seconds: u64,
    battery_level: i32,
    battery_status: BatteryStatus,
    battery_temp: Option<f32>,
    brightness: f32,
    refresh_rate: f32,
}

// ---------------------------------------------------------------------------
// Hot monitoring loop — spawned once, runs forever with auto-recovery.
// ---------------------------------------------------------------------------

pub async fn run_monitor(
    tx: watch::Sender<SystemStats>,
    paths: DevicePaths,
    static_info: Arc<StaticDeviceInfo>,
    health: Arc<AtomicHealth>,
    svc_status: Arc<ServiceStatus>,
    poll_interval: Duration,
) {
    let core_len = static_info.cores.len();

    // Pre-allocated scratch space — reused across ticks and rish restarts.
    let mut core_snaps: Vec<CpuSnap> = (0..core_len).map(|_| CpuSnap::default()).collect();
    let mut core_usages = vec![0.0_f32; core_len];

    let mut tick: u64 = 0;
    let mut cached_storage = StorageInfo { free_gb: 0.0, total_gb: 0.0 };
    let mut retries = 0_u32;

    loop {
        // ── Spawn rish session ───────────────────────────────────────
        let mut child = match spawn_rish() {
            Some(c) => c,
            None => {
                retries += 1;
                svc_status.rish_retry_count.store(retries, Ordering::Relaxed);
                if retries > MAX_RISH_RETRIES {
                    error!(
                        retries = retries,
                        sessions = svc_status.rish_session_count.load(Ordering::Relaxed),
                        ticks = tick,
                        "rish retry limit exhausted — monitor dead"
                    );
                    health.store(MonitorHealth::Dead);
                    return;
                }
                health.store(MonitorHealth::Degraded);
                warn!(
                    attempt = retries,
                    max = MAX_RISH_RETRIES,
                    delay_secs = RISH_RETRY_DELAY.as_secs(),
                    "rish spawn failed — is Shizuku running? retrying"
                );
                tokio::time::sleep(RISH_RETRY_DELAY).await;
                continue;
            }
        };

        let mut stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let mut lines = BufReader::new(stdout).lines();

        let session = svc_status.rish_session_count.fetch_add(1, Ordering::Relaxed) + 1;
        svc_status.rish_retry_count.store(0, Ordering::Relaxed);
        health.store(MonitorHealth::Healthy);
        retries = 0;
        info!(session = session, ticks_so_far = tick, "rish connected — monitor healthy");

        // ── Polling loop (runs until rish dies) ──────────────────────
        loop {
            let is_storage_tick = tick.is_multiple_of(STORAGE_TICK_INTERVAL);

            // Direct sysfs/procfs reads. These are kernel memory-mapped and
            // return in single-digit µs, so synchronous reads are appropriate
            // here — spawning blocking tasks would add more overhead than the
            // reads themselves.
            let cpu_temp = read_sysfs_thermal(&paths.cpu_temp);
            let gpu_temp = read_sysfs_thermal(&paths.gpu_temp);
            let gpu_load = read_gpu_load();
            let mem = read_memory();
            let cur_freqs = read_cpu_freqs(core_len);

            // Track sysfs availability and log transitions so path-miss issues
            // are immediately visible without enabling debug verbosity.
            let prev_cpu_ok = svc_status.cpu_temp_ok.swap(cpu_temp.is_some(), Ordering::Relaxed);
            if prev_cpu_ok && cpu_temp.is_none() {
                warn!(path = %paths.cpu_temp, tick = tick, "cpu thermal sysfs read failed — check path");
            } else if !prev_cpu_ok && cpu_temp.is_some() && tick > 0 {
                info!(path = %paths.cpu_temp, tick = tick, "cpu thermal sysfs read recovered");
            }

            let prev_gpu_temp_ok = svc_status.gpu_temp_ok.swap(gpu_temp.is_some(), Ordering::Relaxed);
            if prev_gpu_temp_ok && gpu_temp.is_none() {
                warn!(path = %paths.gpu_temp, tick = tick, "gpu thermal sysfs read failed — check path");
            } else if !prev_gpu_temp_ok && gpu_temp.is_some() && tick > 0 {
                info!(path = %paths.gpu_temp, tick = tick, "gpu thermal sysfs read recovered");
            }

            // gpu_load is expected absent on non-Qualcomm SoCs — keep at debug level.
            let prev_gpu_load_ok = svc_status.gpu_load_ok.swap(gpu_load.is_some(), Ordering::Relaxed);
            if prev_gpu_load_ok && gpu_load.is_none() {
                debug!(tick = tick, "kgsl gpu load sysfs node became unavailable");
            } else if !prev_gpu_load_ok && gpu_load.is_some() && tick > 0 {
                info!(tick = tick, "kgsl gpu load sysfs node became available");
            }

            // Storage uses `statvfs` which can block on slow mounts.
            if is_storage_tick {
                cached_storage = tokio::task::spawn_blocking(read_storage)
                    .await
                    .unwrap_or_else(|e| {
                        warn!(%e, "storage read task panicked");
                        StorageInfo { free_gb: 0.0, total_gb: 0.0 }
                    });
                debug!(
                    free_gb = cached_storage.free_gb,
                    total_gb = cached_storage.total_gb,
                    tick = tick,
                    "storage stats refreshed"
                );
            }

            // Write command batch to rish.
            if stdin.write_all(RISH_CMD).await.is_err()
                || stdin.flush().await.is_err()
            {
                error!(tick = tick, session = svc_status.rish_session_count.load(Ordering::Relaxed), "rish stdin pipe broken — restarting");
                break;
            }

            // Parse rish output.
            let Some(rish) = parse_rish_tick(
                &mut lines,
                &mut core_snaps,
                &mut core_usages,
            )
            .await
            else {
                error!(tick = tick, session = svc_status.rish_session_count.load(Ordering::Relaxed), "rish output stream ended — restarting");
                break;
            };

            // ── Build payload ────────────────────────────────────────
            let cores: Vec<CoreData> = static_info
                .cores
                .iter()
                .enumerate()
                .map(|(i, info)| CoreData {
                    name: Arc::clone(&info.name),
                    usage: core_usages.get(i).copied().unwrap_or(0.0),
                    model_name: Arc::clone(&info.model_name),
                    cur_freq: cur_freqs.get(i).copied().unwrap_or(0.0),
                    min_freq: info.min_freq,
                    max_freq: info.max_freq,
                })
                .collect();

            let stats = SystemStats {
                device: Arc::clone(&static_info.device),
                uptime_seconds: rish.uptime_seconds,
                battery_level: rish.battery_level,
                battery_status: rish.battery_status,
                battery_temp: rish.battery_temp,
                cpu_temp,
                gpu_temp,
                gpu_load,
                memory_used_mb: (mem.total_mb - mem.available_mb).max(0.0),
                memory_total_mb: mem.total_mb,
                swap_used_mb: (mem.swap_total_mb - mem.swap_free_mb).max(0.0),
                swap_total_mb: mem.swap_total_mb,
                storage_free_gb: cached_storage.free_gb,
                storage_total_gb: cached_storage.total_gb,
                refresh_rate: rish.refresh_rate,
                brightness: rish.brightness,
                cores,
            };

            let _ = tx.send(stats);
            tick += 1;
            // Sync tick counter and timestamp into shared status for /debug endpoint.
            svc_status.tick_count.store(tick, Ordering::Relaxed);
            svc_status.last_tick_unix_secs.store(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
            debug!(
                tick = tick,
                battery = rish.battery_level,
                uptime_s = rish.uptime_seconds,
                "tick dispatched"
            );
            // Periodic info-level heartbeat so the service produces visible output
            // even when there is nothing unusual to report.
            if tick % 500 == 0 {
                info!(
                    tick = tick,
                    session = svc_status.rish_session_count.load(Ordering::Relaxed),
                    "monitor heartbeat"
                );
            }
            tokio::time::sleep(poll_interval).await;
        }

        // ── Cleanup dead rish child ──────────────────────────────────
        let _ = child.start_kill();
        let _ = child.wait().await;

        retries += 1;
        svc_status.rish_retry_count.store(retries, Ordering::Relaxed);
        if retries > MAX_RISH_RETRIES {
            error!(
                retries = retries,
                sessions = svc_status.rish_session_count.load(Ordering::Relaxed),
                ticks = tick,
                "rish retry limit exhausted — monitor dead"
            );
            health.store(MonitorHealth::Dead);
            return;
        }
        health.store(MonitorHealth::Degraded);
        warn!(
            attempt = retries,
            max = MAX_RISH_RETRIES,
            delay_secs = RISH_RETRY_DELAY.as_secs(),
            tick = tick,
            "rish session lost — is Shizuku still running? restarting"
        );
        tokio::time::sleep(RISH_RETRY_DELAY).await;
    }
}

// ---------------------------------------------------------------------------
// Rish lifecycle
// ---------------------------------------------------------------------------

/// Spawn a long-lived `rish` shell with piped stdin/stdout.
fn spawn_rish() -> Option<Child> {
    Command::new("rish")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .map_err(|e| error!(%e, "failed to spawn rish"))
        .ok()
}

/// Parse one batch of rish output.  Returns `None` if the stream ended.
async fn parse_rish_tick(
    lines: &mut RishLines,
    core_snaps: &mut [CpuSnap],
    core_usages: &mut [f32],
) -> Option<RishTick> {
    core_usages.iter_mut().for_each(|u| *u = 0.0);
    let core_len = core_snaps.len();

    let mut tick = RishTick {
        uptime_seconds: 0,
        battery_level: 0,
        battery_status: BatteryStatus::Unknown,
        battery_temp: None,
        brightness: 0.0,
        refresh_rate: 0.0,
    };

    let mut in_display = false;
    let mut brightness_found = false;
    let mut refresh_rate_found = false;

    loop {
        let raw_line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) | Err(_) => return None,
        };
        let line = raw_line.trim();

        if line == "END_OF_BATCH" {
            return Some(tick);
        }

        // ── Section markers ──────────────────────────────────────────
        if line == "DISPLAY_DATA" {
            in_display = true;
            continue;
        }
        if line == "DISPLAY_END" {
            in_display = false;
            continue;
        }

        // ── Display section ──────────────────────────────────────────
        if in_display {
            if let Some(val) = line.strip_prefix("mBrightness=")
                && !brightness_found
            {
                tick.brightness = val.parse().unwrap_or(0.0);
                brightness_found = true;
            } else if let Some(val) = line.strip_prefix("mActiveRenderFrameRate=")
                && !refresh_rate_found
            {
                tick.refresh_rate = val.parse().unwrap_or(0.0);
                refresh_rate_found = true;
            }
            continue;
        }

        // ── Normal section (uptime / cpu / battery) ──────────────────
        let (tag, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));

        match tag {
            "UPTIME" => {
                tick.uptime_seconds = rest
                    .split_whitespace()
                    .next()
                    .and_then(|v| v.parse::<f32>().ok())
                    .unwrap_or(0.0) as u64;
            }
            "cpu" => { /* aggregate line — skip */ }
            "level:" => tick.battery_level = rest.trim().parse().unwrap_or(0),
            "status:" => {
                tick.battery_status =
                    BatteryStatus::from_code(rest.trim().parse().unwrap_or(0));
            }
            "temperature:" => {
                tick.battery_temp = Some(parse_or_zero(rest.trim()) / 10.0);
            }
            tag if tag.starts_with("cpu") => {
                if let Ok(idx) = tag[3..].parse::<usize>()
                    && idx < core_len
                {
                    let (t, i) = parse_cpu_stat(rest);
                    let dt = t.saturating_sub(core_snaps[idx].total);
                    let di = i.saturating_sub(core_snaps[idx].idle);
                    if dt > 0 {
                        core_usages[idx] = (dt - di) as f32 / dt as f32 * 100.0;
                    }
                    core_snaps[idx] = CpuSnap { total: t, idle: i };
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Direct sysfs/procfs readers — no privilege needed.
// ---------------------------------------------------------------------------

/// Read a thermal zone temperature in °C.  Returns `None` if the sysfs
/// file is absent or unparseable, so callers see `null` instead of a
/// misleading `0.0`.
fn read_sysfs_thermal(path: &str) -> Option<f32> {
    let raw: f32 = std::fs::read_to_string(path)
        .ok()?
        .trim()
        .parse()
        .ok()?;
    Some(raw / 1000.0)
}

/// Read GPU load from kgsl sysfs.  Returns `None` when the sysfs node
/// is unavailable (e.g. non-Qualcomm SoCs).
fn read_gpu_load() -> Option<f32> {
    let content = std::fs::read_to_string("/sys/class/kgsl/kgsl-3d0/gpubusy").ok()?;
    let mut it = content.split_whitespace();
    let busy: u64 = it.next()?.parse().ok()?;
    let total: u64 = it.next()?.parse().ok()?;
    if total > 0 { Some(busy as f32 / total as f32 * 100.0) } else { None }
}

/// Read memory stats from `/proc/meminfo`.
fn read_memory() -> MemoryInfo {
    let content = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let mut info = MemoryInfo {
        total_mb: 0.0,
        available_mb: 0.0,
        swap_total_mb: 0.0,
        swap_free_mb: 0.0,
    };
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            info.total_mb = parse_or_zero(rest.trim()) / 1024.0;
        } else if let Some(rest) = line.strip_prefix("MemAvailable:") {
            info.available_mb = parse_or_zero(rest.trim()) / 1024.0;
        } else if let Some(rest) = line.strip_prefix("SwapTotal:") {
            info.swap_total_mb = parse_or_zero(rest.trim()) / 1024.0;
        } else if let Some(rest) = line.strip_prefix("SwapFree:") {
            info.swap_free_mb = parse_or_zero(rest.trim()) / 1024.0;
        }
    }
    info
}

/// Read current frequency for each core from sysfs, in MHz.
fn read_cpu_freqs(count: usize) -> Vec<f32> {
    (0..count)
        .map(|i| {
            std::fs::read_to_string(format!(
                "/sys/devices/system/cpu/cpu{i}/cpufreq/scaling_cur_freq"
            ))
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .unwrap_or(0.0)
                / 1000.0
        })
        .collect()
}

/// Read storage free/total for `/data` via `statvfs`.
///
/// Called through [`tokio::task::spawn_blocking`] because `statvfs` can
/// block on slow or remote mounts.
fn read_storage() -> StorageInfo {
    let path = CString::new("/data").expect("/data is a valid C string");

    // SAFETY: `statvfs` is called with a valid NUL-terminated path pointing to
    // an existing mount point.  The `stat` struct is zero-initialized and
    // written atomically by the kernel.  No aliasing or lifetime issues exist.
    unsafe {
        let mut stat: libc::statvfs = std::mem::zeroed();
        if libc::statvfs(path.as_ptr(), &mut stat) == 0 {
            let bs = stat.f_frsize as f64;
            let gb = 1024.0 * 1024.0 * 1024.0;
            StorageInfo {
                free_gb: (stat.f_bavail as f64 * bs / gb) as f32,
                total_gb: (stat.f_blocks as f64 * bs / gb) as f32,
            }
        } else {
            StorageInfo { free_gb: 0.0, total_gb: 0.0 }
        }
    }
}

// ---------------------------------------------------------------------------
// Parsing helpers
// ---------------------------------------------------------------------------

/// Parse the first whitespace-delimited token as `f32`, defaulting to `0.0`.
fn parse_or_zero(s: &str) -> f32 {
    s.split_whitespace()
        .next()
        .and_then(|v| v.parse().ok())
        .unwrap_or(0.0)
}

/// Parse a `/proc/stat` CPU line's numeric fields into (total, idle).
fn parse_cpu_stat(rest: &str) -> (u64, u64) {
    let mut total = 0_u64;
    let mut idle = 0_u64;
    for (i, tok) in rest.split_whitespace().take(8).enumerate() {
        if let Ok(v) = tok.parse::<u64>() {
            total += v;
            // Fields 3 = idle, 4 = iowait.
            if i == 3 || i == 4 {
                idle += v;
            }
        }
    }
    (total, idle)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_or_zero_valid() {
        assert_eq!(parse_or_zero("42.5 kB"), 42.5);
        assert_eq!(parse_or_zero("100"), 100.0);
    }

    #[test]
    fn parse_or_zero_empty_and_garbage() {
        assert_eq!(parse_or_zero(""), 0.0);
        assert_eq!(parse_or_zero("   "), 0.0);
        assert_eq!(parse_or_zero("abc xyz"), 0.0);
    }

    #[test]
    fn parse_cpu_stat_normal_line() {
        // user nice system idle iowait irq softirq steal
        let (total, idle) = parse_cpu_stat("1000 200 300 5000 100 50 20 10");
        assert_eq!(total, 1000 + 200 + 300 + 5000 + 100 + 50 + 20 + 10);
        assert_eq!(idle, 5000 + 100); // idle + iowait
    }

    #[test]
    fn parse_cpu_stat_empty() {
        let (total, idle) = parse_cpu_stat("");
        assert_eq!(total, 0);
        assert_eq!(idle, 0);
    }

    #[test]
    fn parse_cpu_stat_partial() {
        // Only 2 fields — idle (idx 3) and iowait (idx 4) are absent.
        let (total, idle) = parse_cpu_stat("100 200");
        assert_eq!(total, 300);
        assert_eq!(idle, 0);
    }
}