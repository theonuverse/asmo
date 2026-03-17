# Asmo

A lightweight REST API server that exposes real-time Android device stats over HTTP — built in Rust for [Termux](https://termux.dev).

Asmo polls hardware telemetry every 500 ms via [Shizuku](https://shizuku.rikka.app/) (`rish`) and serves it as a clean JSON API. Every metric has its own endpoint — query everything at once, or drill into exactly the data you need.

## What it reports

| Category | Fields | Source | Refresh |
|---|---|---|---|
| **Device** | Manufacturer, product model, SoC model | `getprop` | Static |
| **System** | Kernel version, Android version, uptime | `uname -r` / `getprop` | Static / 500ms |
| **Memory** | Used / total, swap used / total | `/proc/meminfo` | 500ms |
| **Thermal** | CPU temperature, GPU temperature | sysfs thermal zones | 500ms |
| **Battery** | Level, status, temperature | `dumpsys battery` via rish | 500ms |
| **GPU** | Load percentage | sysfs kgsl | 500ms |
| **Storage** | Free / total GB | `statvfs("/data")` | 30s |
| **Display** | Refresh rate, brightness | `dumpsys display` via rish | 500ms |
| **Per-core CPU** | Usage %, current / min / max frequency, model name | sysfs / `/proc/stat` | 500ms |

## API Reference

All endpoints use **GET** requests.

### Discovery

| Endpoint | Description |
|---|---|
| `/` | API index — lists every available endpoint |
| `/stats` | Full system stats snapshot |

### Single fields

Every top-level field in the stats is its own endpoint:

| Endpoint | Returns |
|---|---|
| `/manufacturer` | `{"manufacturer": "Nothing"}` |
| `/product_model` | `{"product_model": "A065"}` |
| `/soc_model` | `{"soc_model": "SM8475"}` |
| `/kernel_version` | `{"kernel_version": "5.10.198-..."}` |
| `/android_version` | `{"android_version": "15"}` |
| `/uptime_seconds` | `{"uptime_seconds": 8928}` |
| `/battery_level` | `{"battery_level": 100}` |
| `/battery_status` | `{"battery_status": "Full"}` |
| `/battery_temp` | `{"battery_temp": 31.0}` |
| `/cpu_temp` | `{"cpu_temp": 34.4}` |
| `/gpu_temp` | `{"gpu_temp": 34.098}` |
| `/gpu_load` | `{"gpu_load": 5.27}` |
| `/memory_used_mb` | `{"memory_used_mb": 5585.789}` |
| `/memory_total_mb` | `{"memory_total_mb": 11260.543}` |
| `/swap_used_mb` | `{"swap_used_mb": 2418.5}` |
| `/swap_total_mb` | `{"swap_total_mb": 4096.0}` |
| `/storage_free_gb` | `{"storage_free_gb": 84.3}` |
| `/storage_total_gb` | `{"storage_total_gb": 236.1}` |
| `/refresh_rate` | `{"refresh_rate": 120.0}` |
| `/brightness` | `{"brightness": 0.212}` |

### Per-core CPU

| Endpoint | Description |
|---|---|
| `/cores` | All cores (full array) |
| `/cores/cpu0` | Full snapshot of core 0 |
| `/cores/cpu0/usage` | `{"usage": 28.57}` |
| `/cores/cpu0/model_name` | `{"model_name": "Cortex-A510"}` |
| `/cores/cpu0/cur_freq` | `{"cur_freq": 1804.8}` |
| `/cores/cpu0/min_freq` | `{"min_freq": 300.0}` |
| `/cores/cpu0/max_freq` | `{"max_freq": 1804.8}` |

> Replace `cpu0` with any core name (`cpu1`, `cpu2`, … `cpu7`, etc.).

### Multi-field queries

Combine fields with commas to fetch multiple values in one request. **Fields are returned in the order you specify:**

| Endpoint | Returns |
|---|---|
| `/battery_level,battery_status` | `{"battery_level": 100, "battery_status": "Full"}` |
| `/cpu_temp,gpu_temp,gpu_load` | `{"cpu_temp": 34.4, "gpu_temp": 34.1, "gpu_load": 5.27}` |
| `/manufacturer,product_model,soc_model` | `{"manufacturer": "Nothing", "product_model": "A065", "soc_model": "SM8475"}` |
| `/cores/cpu0/usage,cur_freq` | `{"usage": 28.57, "cur_freq": 1804.8}` |
| `/cores/cpu0/usage,name,model_name` | `{"usage": 28.57, "name": "cpu0", "model_name": "Cortex-A510"}` |

> Works at any level — top-level fields, or fields within a specific core.

### Wildcards

Use `*` or `all` to query a field from **every** item in an array. Each result includes the core's `name` for identification:

| Endpoint | Description |
|---|---|
| `/cores/*/usage` | Usage of every core |
| `/cores/all/usage` | Same — shell-friendly alias for `*` |
| `/cores/*/usage,cur_freq` | Usage + frequency of every core |
| `/cores/all/cur_freq,model_name` | Frequency + model of every core |

> **Shell note:** `*` requires quoting in shell: `curl -s 'localhost:3000/cores/*/usage'`
> `all` needs no quoting: `curl -s localhost:3000/cores/all/usage`

### Dynamic routing

Endpoints are **generated automatically** from the data structure. If a new field is added to the stats in code, it becomes a reachable endpoint immediately — no routing changes required.

### Error responses

Unknown paths return `404` with a helpful JSON body:

```json
{
  "error": "not found",
  "path": "/nonexistent",
  "hint": "GET / for available endpoints"
}
```

### Design note: scope of comma queries

Comma-separated fields work within a **single path level** — for example `/battery_level,cpu_temp` (top-level) or `/cores/cpu0/usage,cur_freq` (within a core). Mixing different path depths (like `/gpu_load,cores/all/usage`) is not possible because `/` is the HTTP path separator, meaning the server would interpret it as nested segments rather than separate fields.

For mixed-depth queries, use `/stats` with jq:

```sh
curl -s localhost:3000/stats | jq '{gpu_load, cores: [.cores[] | {name, usage}]}'
```

<details>
<summary>Full /stats response example</summary>

```json
{
  "manufacturer": "Nothing",
  "product_model": "A065",
  "soc_model": "SM8475",
  "kernel_version": "5.10.198-...",
  "android_version": "15",
  "uptime_seconds": 8928,
  "battery_level": 100,
  "battery_status": "Full",
  "battery_temp": 31,
  "cpu_temp": 34.4,
  "gpu_temp": 34.098,
  "gpu_load": 5.2692976,
  "memory_used_mb": 5585.789,
  "memory_total_mb": 11260.543,
  "swap_used_mb": 2418.5,
  "swap_total_mb": 4096.0,
  "storage_free_gb": 84.3,
  "storage_total_gb": 236.1,
  "refresh_rate": 120.0,
  "brightness": 0.212,
  "cores": [
    {
      "name": "cpu0",
      "usage": 28.57143,
      "model_name": "Cortex-A510",
      "cur_freq": 1804.8,
      "min_freq": 300,
      "max_freq": 1804.8
    },
    {
      "name": "cpu1",
      "usage": 28.57143,
      "model_name": "Cortex-A510",
      "cur_freq": 1440,
      "min_freq": 300,
      "max_freq": 1804.8
    },
    {
      "name": "cpu2",
      "usage": 26.984129,
      "model_name": "Cortex-A510",
      "cur_freq": 1440,
      "min_freq": 300,
      "max_freq": 1804.8
    },
    {
      "name": "cpu3",
      "usage": 31.746033,
      "model_name": "Cortex-A510",
      "cur_freq": 1440,
      "min_freq": 300,
      "max_freq": 1804.8
    },
    {
      "name": "cpu4",
      "usage": 9.230769,
      "model_name": "Cortex-A710",
      "cur_freq": 1766.4,
      "min_freq": 633.6,
      "max_freq": 2496
    },
    {
      "name": "cpu5",
      "usage": 23.188406,
      "model_name": "Cortex-A710",
      "cur_freq": 1881.6,
      "min_freq": 633.6,
      "max_freq": 2496
    },
    {
      "name": "cpu6",
      "usage": 10.769231,
      "model_name": "Cortex-A710",
      "cur_freq": 1881.6,
      "min_freq": 633.6,
      "max_freq": 2496
    },
    {
      "name": "cpu7",
      "usage": 0,
      "model_name": "Cortex-X2",
      "cur_freq": 2476.8,
      "min_freq": 787.2,
      "max_freq": 2995.2
    }
  ]
}
```

<img src="assets/showcase.png" alt="Asmo showcase" width=250>

</details>

## Prerequisites

- [Termux](https://termux.dev) installed
- [Shizuku](https://shizuku.rikka.app/) running (provides `rish` for privileged sysfs access)
- `rish` must be callable from Termux (`rish -c 'echo ok'` must work)

> **Hard requirement:** asmo will **not start** without a working `rish` session.

## Build from source

```sh
# Update packages
yes | pkg up

# Install dependencies
pkg install git rust -y

# Clone the repo
git clone https://github.com/theonuverse/asmo.git
cd asmo

# Build
cargo build --release

# Install into Termux PATH
cp target/release/asmo $PREFIX/bin/
```

> **Tip:** If you just want to test without installing, run `cargo run --release` from the project directory.

## Running as a Termux service

The included `sv_setup.sh` is intentionally service-only. It does **not** clone, build, or install dependencies.

Run it from the project directory (after your normal build/install flow):

```sh
./sv_setup.sh
```

`sv_setup.sh` creates the runit files under `$PREFIX/etc/sv/asmo`, enables the service and starts it.

> **Prerequisites:** [Shizuku](https://shizuku.rikka.app/) must be running and `rish` must work before setup. `termux-services` must be available (the script installs it if missing and asks for one Termux restart).

The service runner uses `exec script -q -c "asmo" /dev/null 2>&1` so `rish` has a tty.

## Termux service management

All service control uses the standard `sv` tool from runit.

### Core commands

| Command | Effect |
|---|---|
| `sv status asmo` | Show current state: `run`, `down`, or `finish` |
| `sv up asmo` | Start the service if it is stopped |
| `sv down asmo` | Stop the service gracefully |
| `sv restart asmo` | Graceful stop then restart |
| `sv once asmo` | Start once, do not restart on exit |

### Boot persistence

```sh
sv-enable asmo    # start automatically on every Termux session open (done by sv_setup.sh)
sv-disable asmo   # remove from auto-start
```

`sv-enable` registers the service with runsvdir by creating a symlink under `$PREFIX/var/service/`. The daemon watches that directory and automatically restarts asmo if it crashes.

### Viewing logs

asmo logs everything through the runit log pipeline. Logs are rotated automatically by `svlogd`.

```sh
# Follow live output (the active log file is always named 'current')
tail -f $PREFIX/var/log/asmo/current

# Show the last 50 lines
tail -50 $PREFIX/var/log/asmo/current

# Search for errors
grep -i error $PREFIX/var/log/asmo/current

# List all rotated log segments
ls $PREFIX/var/log/asmo/
```

The log runner script uses `svlogd -tt` which prefixes every line with a human-readable timestamp — no extra tools needed.

### Controlling log verbosity

asmo reads `RUST_LOG` to set verbosity. The default is `info`. To change it, edit the run script:

```sh
nano $PREFIX/etc/sv/asmo/run
```

The script looks like this — uncomment the `RUST_LOG=debug` line:

```sh
#!/data/data/com.termux/files/usr/bin/sh
exec script -q -c "RUST_LOG=debug asmo" /dev/null 2>&1
```

Then restart:

```sh
sv restart asmo
```

Available levels from quietest to most verbose: `error`, `warn`, `info` (default), `debug`.

### Service file locations

| Path | Purpose |
|---|---|
| `$PREFIX/etc/sv/asmo/run` | Service start script |
| `$PREFIX/etc/sv/asmo/log/run` | Log pipeline script |
| `$PREFIX/var/log/asmo/current` | Active log file |
| `$PREFIX/var/service/asmo` | Symlink that registers the service with runsvdir |

### Changing configuration

Pass CLI flags inside the run script. Edit it and restart:

```sh
nano $PREFIX/etc/sv/asmo/run
```

```sh
#!/data/data/com.termux/files/usr/bin/sh
exec script -q -c "asmo --port 8080 --bind 127.0.0.1 --interval 200" /dev/null 2>&1
```

```sh
sv restart asmo
```

## Usage

```sh
asmo
```

```
🚀 Asmo running on: http://192.168.1.42:3000
   GET / for all available endpoints
```

Open the printed URL from any device on the same network.

## Examples

### Discover all endpoints

```sh
curl -s localhost:3000/ | jq .
```

### Full stats

```sh
curl -s localhost:3000/stats | jq .
```

### Single fields

```sh
curl -s localhost:3000/battery_level
# → {"battery_level":100}

curl -s localhost:3000/gpu_load
# → {"gpu_load":5.27}

curl -s localhost:3000/cpu_temp
# → {"cpu_temp":34.4}
```

### Multiple fields at once

Fields are returned in the order you specify.

```sh
# Your order: usage first, then name, then model
curl -s localhost:3000/cores/cpu0/usage,name,model_name | jq .
# → {"usage":28.57,"name":"cpu0","model_name":"Cortex-A510"}

# Device identity
curl -s localhost:3000/manufacturer,product_model,soc_model | jq .
# → {"manufacturer":"Nothing","product_model":"A065","soc_model":"SM8475"}

# Thermal overview
curl -s localhost:3000/cpu_temp,gpu_temp,gpu_load | jq .
# → {"cpu_temp":34.4,"gpu_temp":34.1,"gpu_load":5.27}

# Battery summary
curl -s localhost:3000/battery_level,battery_status,battery_temp | jq .
# → {"battery_level":100,"battery_status":"Full","battery_temp":31.0}
```

### Per-core CPU data

```sh
# Full snapshot of cpu0
curl -s localhost:3000/cores/cpu0 | jq .

# Just the usage of cpu0
curl -s localhost:3000/cores/cpu0/usage
# → {"usage":28.57}

# Current frequency of cpu4
curl -s localhost:3000/cores/cpu4/cur_freq
# → {"cur_freq":1766.4}
```

### Wildcard queries

```sh
# Usage of every core (* requires quoting in shell)
curl -s 'localhost:3000/cores/*/usage' | jq .
```

```json
[
  {"name": "cpu0", "usage": 28.57},
  {"name": "cpu1", "usage": 14.29},
  {"name": "cpu2", "usage": 26.98},
  "..."
]
```

```sh
# Shell-friendly alternative — no quoting needed
curl -s localhost:3000/cores/all/usage | jq .

# Multiple fields from every core — your field order is preserved
curl -s localhost:3000/cores/all/usage,cur_freq,model_name | jq .
```

```json
[
  {"name": "cpu0", "usage": 28.57, "cur_freq": 1804.8, "model_name": "Cortex-A510"},
  {"name": "cpu1", "usage": 14.29, "cur_freq": 1440.0, "model_name": "Cortex-A510"},
  "..."
]
```

### Monitor continuously

```sh
# Poll battery every 2 seconds
watch -n2 'curl -s localhost:3000/battery_level'

# Poll GPU load + temps
watch -n2 'curl -s localhost:3000/cpu_temp,gpu_temp,gpu_load'
```

### Scripting / piping

```sh
# Log battery level to a file
while true; do curl -s localhost:3000/battery_level >> battery.jsonl; sleep 5; done

# Get all core usages in one line
for i in $(seq 0 7); do
  echo -n "cpu$i: "
  curl -s localhost:3000/cores/cpu$i/usage | jq -r '.usage'
done
```

### Integration

The API is stateless and side-effect-free — pipe the JSON into `jq`, `awk`, `gnuplot`, Grafana, Home Assistant, Discord webhooks, or anything else that consumes JSON.

## Architecture

```
main.rs        → Entrypoint — binds the HTTP server (Axum) on port 3000
router.rs      → Dynamic router — resolves any URL path to a stats field at runtime
discover.rs    → One-shot device probe at startup (thermal zones, core topology, SoC identity)
monitor.rs     → Async polling loop — sysfs reads + rish for privileged data (battery, uptime, /proc/stat, network, display)
types.rs       → Shared data structures (zero-copy Arc<str> strings, typed BatteryStatus enum)
```

### How dynamic routing works

1. `SystemStats` is serialized into a `serde_json::Value` tree on each request.
2. The URL path (`/cores/cpu0/usage`) is split into segments: `["cores", "cpu0", "usage"]`.
3. Each segment navigates one level deeper — object fields by key, array items by `"name"`, wildcards (`*`/`all`) expand over all array items.
4. Comma-separated last segments resolve multiple fields, preserving your specified order.
5. The resolved value is returned as JSON.

This means **any new field** added to `SystemStats` (or its nested structs) is instantly available as an endpoint — no manual route registration, no boilerplate.

## Debugging & troubleshooting

### Quick health check

```sh
# Monitor health from the API
curl -s localhost:3000/health | jq .
# → {"status":"healthy"}  (or "degraded" / "dead" / "starting")

# Deep diagnostics endpoint
curl -s localhost:3000/debug | jq .
```

The `/debug` endpoint exposes the full runtime state:

```json
{
  "asmo_version": "0.5.0",
  "uptime_secs": 3812,
  "bind_addr": "0.0.0.0:3000",
  "poll_interval_ms": 500,
  "core_count": 8,
  "monitor": {
    "health": "healthy",
    "rish_retry_count": 0,
    "rish_session_count": 1,
    "tick_count": 7624,
    "last_tick_age_secs": 0
  },
  "sysfs": {
    "cpu_temp_path": "/sys/class/thermal/thermal_zone12/temp",
    "cpu_temp_ok": true,
    "gpu_temp_path": "/sys/class/thermal/thermal_zone18/temp",
    "gpu_temp_ok": true,
    "gpu_load_ok": true
  }
}
```

### Common issues

#### Service won’t start or stays `down`

```sh
# Check supervision state
sv status asmo

# Read the startup log
tail -20 $PREFIX/var/log/asmo/current

# Try running the binary directly to see raw errors
asmo
```

If `asmo: command not found`, the binary was not installed to `$PREFIX/bin/`. Re-run:

```sh
cargo build --release
cp target/release/asmo $PREFIX/bin/asmo
./sv_setup.sh
```

#### `rish` not available / service exits immediately

asmo requires [Shizuku](https://shizuku.rikka.app/) and a working `rish` session before startup.
If `rish` is unavailable, asmo exits immediately by design.

```sh
# Test rish access directly
rish -c 'echo rish ok'

# If that fails: open Shizuku, tap Pairing, and authorize Termux.
# Then restart asmo once rish works.
```

Watch retries in the log:

```sh
tail -f $PREFIX/var/log/asmo/current | grep -i rish
```

#### CPU or GPU temperature shows `null`

The thermal sysfs paths are auto-detected at startup. Get the chosen paths and their status:

```sh
curl -s localhost:3000/debug | jq .sysfs
```

List all thermal zones to find the right one for your device:

```sh
for z in /sys/class/thermal/thermal_zone*/; do
  printf "%s  type=%s\n" "$z" "$(cat ${z}type 2>/dev/null)"
done
```

#### Service stopped producing data

```sh
# Check health and tick count
curl -s localhost:3000/debug | jq .monitor

# Watch tick_count — should increase every ~500 ms
watch -n1 'curl -s localhost:3000/debug | jq .monitor.tick_count'
```

If `last_tick_age_secs` is growing and health is `degraded`, asmo is failing to keep rish connected. Check Shizuku.

#### Enable full debug logging

```sh
# Add RUST_LOG=debug before the exec line
sed -i 's|^exec asmo|export RUST_LOG=debug\nexec asmo|' $PREFIX/etc/sv/asmo/run
sv restart asmo
tail -f $PREFIX/var/log/asmo/current
```

To revert:

```sh
sed -i '/^export RUST_LOG/d' $PREFIX/etc/sv/asmo/run
sv restart asmo
```

### Troubleshooting checklist

Work through these in order when something is wrong:

1. `sv status asmo` — is runit supervising the service?
2. `tail -20 $PREFIX/var/log/asmo/current` — what did asmo log at startup?
3. `curl -s localhost:3000/health` — is the HTTP server responding?
4. `curl -s localhost:3000/debug | jq .` — full internal runtime state
5. `rish -c 'echo ok'` — can asmo reach Shizuku?
6. Enable `RUST_LOG=debug` and restart — every probe, tick, and connection event becomes visible

## License

[MIT](LICENSE)
