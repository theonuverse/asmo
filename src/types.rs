use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use serde::Serialize;

// ---------------------------------------------------------------------------
// Battery status as a proper enum — no raw `&'static str` floating around.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, Default, PartialEq, Eq, Debug)]
pub enum BatteryStatus {
    Charging,
    Discharging,
    #[serde(rename = "Not Charging")]
    NotCharging,
    Full,
    #[default]
    #[serde(rename = "N/A")]
    Unknown,
}

impl BatteryStatus {
    pub fn from_code(code: i32) -> Self {
        match code {
            2 => Self::Charging,
            3 => Self::Discharging,
            4 => Self::NotCharging,
            5 => Self::Full,
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------------------
// Monitor health — exposed via GET /health.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Copy, Default, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MonitorHealth {
    #[default]
    Starting,
    Healthy,
    Degraded,
    Dead,
}

impl MonitorHealth {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Healthy,
            2 => Self::Degraded,
            3 => Self::Dead,
            _ => Self::Starting,
        }
    }

    fn as_u8(self) -> u8 {
        match self {
            Self::Starting => 0,
            Self::Healthy => 1,
            Self::Degraded => 2,
            Self::Dead => 3,
        }
    }
}

/// Atomic wrapper for [`MonitorHealth`], safely shareable between tasks.
#[derive(Default)]
pub struct AtomicHealth(AtomicU8);

impl AtomicHealth {
    pub fn store(&self, health: MonitorHealth) {
        self.0.store(health.as_u8(), Ordering::Relaxed);
    }

    pub fn load(&self) -> MonitorHealth {
        MonitorHealth::from_u8(self.0.load(Ordering::Relaxed))
    }
}

// ---------------------------------------------------------------------------
// Static device strings — built once at discovery, shared via Arc.
// `#[serde(flatten)]` in SystemStats merges these directly into the JSON root,
// eliminating per-tick Arc clones for each individual field.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
pub struct DeviceStrings {
    pub manufacturer: Arc<str>,
    pub product_model: Arc<str>,
    pub soc_model: Arc<str>,
    pub kernel_version: Arc<str>,
    pub android_version: Arc<str>,
}

// ---------------------------------------------------------------------------
// Main stats payload — sent over the watch channel every tick.
// Sensor fields use `Option<f32>`: `None` serializes as JSON `null`,
// clearly distinguishing "unavailable" from a real zero reading.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Default)]
pub struct SystemStats {
    #[serde(flatten)]
    pub device: Arc<DeviceStrings>,

    pub uptime_seconds: u64,
    pub battery_level: i32,
    pub battery_status: BatteryStatus,
    pub battery_temp: Option<f32>,
    pub cpu_temp: Option<f32>,
    pub gpu_temp: Option<f32>,
    pub gpu_load: Option<f32>,
    pub memory_used_mb: f32,
    pub memory_total_mb: f32,
    pub swap_used_mb: f32,
    pub swap_total_mb: f32,
    pub storage_free_gb: f32,
    pub storage_total_gb: f32,
    pub refresh_rate: f32,
    pub brightness: f32,

    pub cores: Vec<CoreData>,
}

// ---------------------------------------------------------------------------
// Per-core snapshot included in every stats payload.
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone)]
pub struct CoreData {
    pub name: Arc<str>,
    pub usage: f32,
    pub model_name: Arc<str>,
    pub cur_freq: f32,
    pub min_freq: f32,
    pub max_freq: f32,
}

// ---------------------------------------------------------------------------
// Discovery-time data — built once, read forever.
// ---------------------------------------------------------------------------

pub struct StaticCoreInfo {
    pub name: Arc<str>,
    pub model_name: Arc<str>,
    pub min_freq: f32,
    pub max_freq: f32,
}

#[derive(Default)]
pub struct CpuSnap {
    pub total: u64,
    pub idle: u64,
}

pub struct StaticDeviceInfo {
    pub device: Arc<DeviceStrings>,
    pub cores: Box<[StaticCoreInfo]>,
}

pub struct DevicePaths {
    pub cpu_temp: Box<str>,
    pub gpu_temp: Box<str>,
}

// ---------------------------------------------------------------------------
// Named return types — replace anonymous tuples for clarity.
// ---------------------------------------------------------------------------

pub struct MemoryInfo {
    pub total_mb: f32,
    pub available_mb: f32,
    pub swap_total_mb: f32,
    pub swap_free_mb: f32,
}

pub struct StorageInfo {
    pub free_gb: f32,
    pub total_gb: f32,
}

// ---------------------------------------------------------------------------
// Runtime service diagnostics — shared between the monitor task and the
// /debug HTTP endpoint so operators can inspect internal state without
// attaching a debugger or tailing raw logs.
// ---------------------------------------------------------------------------

/// Atomic counters and config knobs surfaced by `GET /debug`.
pub struct ServiceStatus {
    /// Consecutive rish restart attempts in the current failure sequence.
    /// Reset to 0 after a successful reconnect.
    pub rish_retry_count: AtomicU32,
    /// Total successful rish sessions opened since startup.
    pub rish_session_count: AtomicU32,
    /// Total monitoring ticks dispatched since startup.
    pub tick_count: AtomicU64,
    /// Unix timestamp (seconds) of the most recent completed tick.
    /// `0` until the first tick completes.
    pub last_tick_unix_secs: AtomicU64,
    /// Unix timestamp (seconds) at which asmo started.
    pub started_unix_secs: u64,
    /// Configured poll interval in milliseconds.
    pub poll_interval_ms: u64,
    /// sysfs path used for CPU temperature reads.
    pub cpu_temp_path: Box<str>,
    /// sysfs path used for GPU temperature reads.
    pub gpu_temp_path: Box<str>,
    /// Whether the most recent CPU temp sysfs read succeeded.
    pub cpu_temp_ok: AtomicBool,
    /// Whether the most recent GPU temp sysfs read succeeded.
    pub gpu_temp_ok: AtomicBool,
    /// Whether the kgsl GPU-load sysfs node is readable on this device.
    /// `false` on non-Qualcomm SoCs — expected and not an error.
    pub gpu_load_ok: AtomicBool,
    /// Configured TCP bind address (e.g. "0.0.0.0:3000").
    pub bind_addr: Box<str>,
    /// Number of CPU cores detected at startup.
    pub core_count: usize,
}

impl ServiceStatus {
    pub fn new(
        started_unix_secs: u64,
        poll_interval_ms: u64,
        cpu_temp_path: &str,
        gpu_temp_path: &str,
        bind_addr: &str,
        core_count: usize,
    ) -> Self {
        Self {
            rish_retry_count: AtomicU32::new(0),
            rish_session_count: AtomicU32::new(0),
            tick_count: AtomicU64::new(0),
            last_tick_unix_secs: AtomicU64::new(0),
            started_unix_secs,
            poll_interval_ms,
            cpu_temp_path: cpu_temp_path.into(),
            gpu_temp_path: gpu_temp_path.into(),
            cpu_temp_ok: AtomicBool::new(false),
            gpu_temp_ok: AtomicBool::new(false),
            gpu_load_ok: AtomicBool::new(false),
            bind_addr: bind_addr.into(),
            core_count,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_status_from_known_codes() {
        assert_eq!(BatteryStatus::from_code(2), BatteryStatus::Charging);
        assert_eq!(BatteryStatus::from_code(3), BatteryStatus::Discharging);
        assert_eq!(BatteryStatus::from_code(4), BatteryStatus::NotCharging);
        assert_eq!(BatteryStatus::from_code(5), BatteryStatus::Full);
    }

    #[test]
    fn battery_status_unknown_for_invalid_codes() {
        for code in [0, 1, 6, -1, 100] {
            assert_eq!(BatteryStatus::from_code(code), BatteryStatus::Unknown);
        }
    }

    #[test]
    fn monitor_health_roundtrip() {
        for h in [
            MonitorHealth::Starting,
            MonitorHealth::Healthy,
            MonitorHealth::Degraded,
            MonitorHealth::Dead,
        ] {
            assert_eq!(MonitorHealth::from_u8(h.as_u8()), h);
        }
    }

    #[test]
    fn atomic_health_store_load() {
        let h = AtomicHealth::default();
        assert_eq!(h.load(), MonitorHealth::Starting);
        h.store(MonitorHealth::Healthy);
        assert_eq!(h.load(), MonitorHealth::Healthy);
    }

    #[test]
    fn device_strings_default_is_empty() {
        let d = DeviceStrings::default();
        assert_eq!(&*d.manufacturer, "");
        assert_eq!(&*d.product_model, "");
    }
}