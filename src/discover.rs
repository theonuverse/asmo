use std::fs;
use std::process::Command;
use std::sync::Arc;

use tracing::{info, warn};

use crate::types::{DevicePaths, DeviceStrings, StaticCoreInfo, StaticDeviceInfo};

// ---------------------------------------------------------------------------
// Discovery-local structs — replace anonymous tuples with named fields.
// ---------------------------------------------------------------------------

struct ThermalProbe {
    cpu_temp_path: String,
    gpu_temp_path: String,
    core_count: usize,
}

struct DeviceIdentity {
    manufacturer: String,
    product_model: String,
    soc_model: String,
}

// ---------------------------------------------------------------------------
// One-shot device discovery — runs at startup, never again.
// ---------------------------------------------------------------------------

pub fn discover_device_layout() -> (DevicePaths, StaticDeviceInfo) {
    let thermal = probe_thermal_and_cores();
    let identity = probe_device_props();
    let (kernel_version, android_version) = probe_system_versions();
    let cores = probe_core_info(thermal.core_count);

    info!(
        cores = cores.len(),
        cpu_thermal = %thermal.cpu_temp_path,
        gpu_thermal = %thermal.gpu_temp_path,
        "device discovery complete"
    );

    let paths = DevicePaths {
        cpu_temp: thermal.cpu_temp_path.into_boxed_str(),
        gpu_temp: thermal.gpu_temp_path.into_boxed_str(),
    };

    let device = Arc::new(DeviceStrings {
        manufacturer: Arc::from(identity.manufacturer),
        product_model: Arc::from(identity.product_model),
        soc_model: Arc::from(identity.soc_model),
        kernel_version: Arc::from(kernel_version),
        android_version: Arc::from(android_version),
    });

    let static_info = StaticDeviceInfo {
        device,
        cores: cores.into_boxed_slice(),
    };

    (paths, static_info)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Probe sysfs thermal zones and CPU topology directly (no `rish` needed).
fn probe_thermal_and_cores() -> ThermalProbe {
    let mut result = ThermalProbe {
        cpu_temp_path: "/sys/class/thermal/thermal_zone0/temp".to_owned(),
        gpu_temp_path: "/sys/class/thermal/thermal_zone1/temp".to_owned(),
        core_count: 0,
    };

    // Scan thermal zones directly from sysfs.
    if let Ok(entries) = fs::read_dir("/sys/class/thermal") {
        let mut zones: Vec<_> = entries
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("thermal_zone"))
            .collect();
        zones.sort_by_key(|e| e.file_name());

        for entry in zones {
            let type_path = entry.path().join("type");
            let Ok(zone_type) = fs::read_to_string(&type_path) else { continue };
            let lower = zone_type.trim().to_ascii_lowercase();
            let temp_path = entry.path().join("temp").to_string_lossy().into_owned();

            if lower.contains("cpuss-0") || lower.contains("aoss-0") {
                result.cpu_temp_path = temp_path;
            } else if lower.contains("gpuss-0") {
                result.gpu_temp_path = temp_path;
            }
        }
    }

    // Count CPU cores directly from sysfs.
    if let Ok(entries) = fs::read_dir("/sys/devices/system/cpu") {
        result.core_count = entries
            .filter_map(Result::ok)
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                s.starts_with("cpu")
                    && s.as_bytes().get(3).is_some_and(|b| b.is_ascii_digit())
            })
            .count();
    }

    result
}

/// Read device identity via Android `getprop`.
fn probe_device_props() -> DeviceIdentity {
    let get = |key| -> String {
        Command::new("getprop")
            .arg(key)
            .output()
            .ok()
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
            .unwrap_or_default()
    };

    DeviceIdentity {
        manufacturer: get("ro.product.manufacturer"),
        product_model: get("ro.product.model"),
        soc_model: get("ro.soc.model"),
    }
}

/// Read kernel and Android version (static, called once at startup).
fn probe_system_versions() -> (String, String) {
    let kernel_version = Command::new("uname")
        .arg("-r")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_else(|| "unknown".to_owned());

    let android_version = Command::new("getprop")
        .arg("ro.build.version.release")
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
        .unwrap_or_default();

    (kernel_version, android_version)
}

/// Gather static per-core info from `lscpu`.
fn probe_core_info(hint: usize) -> Vec<StaticCoreInfo> {
    let output = match Command::new("lscpu")
        .args(["-e=cpu,modelname,minmhz,maxmhz"])
        .output()
    {
        Ok(o) => o,
        Err(e) => {
            warn!(%e, "lscpu not available — core info will be incomplete");
            return Vec::new();
        }
    };

    let raw = String::from_utf8_lossy(&output.stdout);
    parse_lscpu_output(&raw, hint)
}

/// Parse raw `lscpu -e` output into core info entries.
///
/// Extracted from [`probe_core_info`] so the parser can be unit-tested
/// independently of whether `lscpu` is installed.
fn parse_lscpu_output(raw: &str, hint: usize) -> Vec<StaticCoreInfo> {
    let mut cores = Vec::with_capacity(hint);

    for line in raw.lines().skip(1) {
        let mut it = line.split_whitespace();
        let Some(cpu_str) = it.next() else { continue };
        let rest: Vec<&str> = it.collect();
        if rest.len() < 3 {
            continue;
        }

        let model_name = rest[..rest.len() - 2].join(" ");
        let min_freq: f32 = rest[rest.len() - 2].parse().unwrap_or(0.0);
        let max_freq: f32 = rest[rest.len() - 1].parse().unwrap_or(0.0);

        cores.push(StaticCoreInfo {
            name: Arc::from(format!("cpu{cpu_str}").as_str()),
            model_name: Arc::from(model_name.as_str()),
            min_freq,
            max_freq,
        });
    }

    cores.sort_unstable_by(|a, b| {
        let num = |s: &str| s.get(3..).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
        num(&a.name).cmp(&num(&b.name))
    });

    cores
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lscpu_output_normal() {
        let raw = "\
CPU MODELNAME MINMHZ MAXMHZ
0 Cortex-A55 300.0 1804.0
1 Cortex-A55 300.0 1804.0
4 Cortex-A78 710.0 2400.0";

        let cores = parse_lscpu_output(raw, 3);
        assert_eq!(cores.len(), 3);
        assert_eq!(&*cores[0].name, "cpu0");
        assert_eq!(&*cores[1].name, "cpu1");
        assert_eq!(&*cores[2].name, "cpu4");
        assert_eq!(cores[0].min_freq, 300.0);
        assert_eq!(cores[2].max_freq, 2400.0);
    }

    #[test]
    fn parse_lscpu_output_empty() {
        let cores = parse_lscpu_output("CPU MODELNAME MINMHZ MAXMHZ\n", 0);
        assert!(cores.is_empty());
    }

    #[test]
    fn parse_lscpu_output_multiword_model() {
        let raw = "\
CPU MODELNAME MINMHZ MAXMHZ
0 ARM Cortex-A55 rev 1 300.0 1804.0";

        let cores = parse_lscpu_output(raw, 1);
        assert_eq!(cores.len(), 1);
        assert_eq!(&*cores[0].model_name, "ARM Cortex-A55 rev 1");
    }

    #[test]
    fn parse_lscpu_output_sorts_by_core_number() {
        let raw = "\
CPU MODELNAME MINMHZ MAXMHZ
7 Big 710.0 2400.0
0 Little 300.0 1804.0
3 Little 300.0 1804.0";

        let cores = parse_lscpu_output(raw, 3);
        assert_eq!(&*cores[0].name, "cpu0");
        assert_eq!(&*cores[1].name, "cpu3");
        assert_eq!(&*cores[2].name, "cpu7");
    }
}