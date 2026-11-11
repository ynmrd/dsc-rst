# Benchmarks

Targets from the specification: under 150 MB idle RAM where feasible, near-zero idle CPU, startup under 2 s, 60+ FPS. These are targets, not promises.

**No measurement on real Windows hardware, and no comparison with the official client, has been recorded yet.** Everything below comes from a Linux CI-style sandbox without a GPU, where all rendering runs on a CPU rasterizer. Use it as a lower bound for responsiveness and an upper bound for memory, not as the product's real-world numbers.

## Sandbox run, 2026-11-11

| | |
|---|---|
| Build | `cargo build --profile release-fast -p discord-native` (thin LTO; the shipped `release` profile uses fat LTO) |
| Machine | Modal sandbox, 2 vCPU, 8 GB RAM, gVisor kernel, no GPU |
| Display | Xvfb 1440×900, window 1280×760 |
| Renderers | `--software-render`: glow (OpenGL) on Mesa llvmpipe. Default: wgpu on Vulkan, adapter "llvmpipe" (`device_type: Cpu`) |
| Data | `--demo` (offline sample data: 2 servers, ~300 messages in the open channel) |

### Startup, idle CPU, idle memory

`first_frame_ms` is logged by the app (process start to the first built frame). Idle CPU and resident memory were sampled after 15 s, over 60 s with the window open and untouched.

| Renderer | First frame (5 runs) | Idle CPU (60 s) | Resident memory (RSS) | Threads |
|---|---|---|---|---|
| glow / llvmpipe | 156–176 ms | 0.01–0.10 % (3 runs) | 300–310 MB | 12 |
| wgpu Vulkan / llvmpipe | 208–252 ms | 0.00–0.01 % (2 runs) | 410–421 MB | 20 |

- Startup is far below the 2 s target, even with software rendering.
- Idle CPU is effectively zero: the UI draws no frames while nothing changes.
- **Memory is above the 150 MB target in this environment.** A large part of it belongs to the software rasterizer (llvmpipe's JIT and its CPU-side framebuffers and textures), which does not exist on a real GPU. The split between the app and the driver couldn't be measured here because the sandbox kernel doesn't expose `smaps_rollup` or anonymous/file RSS. The real figure has to come from Task Manager on Windows (see below).

### Stress test: 100,000 messages

`discord-native --demo [--software-render] --bench-stress 100000` loads 100k synthetic messages and scrolls continuously for 60 s. "UI build" is the app's own CPU time per frame (layout, virtualization, text shaping). "Frame interval" also includes rasterization, which here is done by the CPU.

| Renderer | Frame interval avg / p95 / p99 / max | FPS | UI build avg / p95 / max | Peak RSS |
|---|---|---|---|---|
| glow / llvmpipe | 17.0 / 21.6 / 25.7 / 54.4 ms | 59 | 4.3 / 5.8 / 34.1 ms | 330 MB |
| wgpu Vulkan / llvmpipe | 21.4 / 29.7 / 53.1 / 95.2 ms | 47 | 4.0 / 5.5 / 8.8 ms | 450 MB |

The app's own frame work (p95 ≈ 5.5–5.8 ms) fits well within the 16.7 ms budget for 60 FPS. The frame rate is limited by CPU rasterization on 2 vCPUs. On a real GPU the frame interval should approach the UI build time plus presentation, but that hasn't been measured yet.

## Still to measure (real hardware)

Follow "Comparison with the official client" in [TEST_PLAN.md](TEST_PLAN.md) on a Windows 10/11 machine with a GPU, using the `release` build from CI, and fill in this table:

| Metric | Discord Native | Official client | Notes |
|---|---|---|---|
| Cold start to visible channel list | — | — | |
| Private working set after 5 min idle | — | — | |
| CPU % idle over 60 s | — | — | |
| CPU % while scrolling a 2,000-message channel | — | — | |
| Stress test p95 frame (Settings → Diagnostics, 100k) | — | n/a | |

## Micro-benchmarks

Criterion benchmarks for the storage, Markdown and cache layers are in `crates/storage/benches`, `crates/messaging/benches` and `crates/cache/benches`. Run them with `cargo bench -p dn-storage --bench storage`, `cargo bench -p dn-messaging --bench markdown` and `cargo bench -p dn-cache --bench store`. Append `-- --quick` for a short run; `--quick` works only together with `--bench <name>`, because the libraries' own test harness rejects it.

Same sandbox, 2026-11-11, `--quick` mode (median of the reported range):

| Benchmark | Median |
|---|---|
| SQLite: insert 50k messages in batches of 100 | 3.30 s (15.2k msg/s) |
| SQLite FTS5 search over 50k: common word / 3-char prefix / two terms / no match | 224 µs / 212 µs / 1.25 ms / 69 µs |
| SQLite search over 50k: common word in one channel / by one author | 0.88 ms / 30.4 ms |
| SQLite: newest 50 in a channel / page of 50 before the middle / latest id | 139 µs / 143 µs / 14 µs |
| SQLite: prune 50k → 25k | 0.90 s |
| Markdown: short message / rich message / 200 KB input | 0.22 µs / 2.86 µs / 3.69 ms |
| Emoji search / `:shortcode:` replacement / split a 10 KB message | 72.6 µs / 0.15 µs / 49.8 µs |
| Store: parse and apply GUILD_CREATE (500 channels, 5k members) | 3.12 ms |
| Store: 10k live MESSAGE_CREATE into the active channel (capped at 3000) | 3.79 ms |
| Store: channel tree of 500 channels / member list of 5k members | 24.8 µs / 535 µs |

The author-filtered search (30 ms) is the slowest interactive path. It runs on the storage thread, not the UI thread.
