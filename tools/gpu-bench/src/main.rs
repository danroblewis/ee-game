//! Minimal wgpu compute benchmark for the parallelism feasibility study.
//!
//! Measures the three things that decide whether a GPU can host this
//! solver: dispatch/synchronisation latency, host<->device transfer cost,
//! and batched small-LU throughput versus the same work on the CPU.
//!
//! Standalone: nothing in crates/ depends on this, and wgpu never enters
//! the shipping lockfile.
//!
//!   cargo run --release --manifest-path tools/gpu-bench/Cargo.toml

use std::time::Instant;
use wgpu::util::DeviceExt;

const N: usize = 8; // per-system matrix order (typical island size is 3-7)
const NN: usize = N * N;
/// 30 Hz tick at dt = 20 us.
const SUBSTEPS_PER_TICK: usize = 1667;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Params {
    count: u32,
    _pad: [u32; 3],
}

fn diag_dominant_batch(b: usize) -> (Vec<f32>, Vec<f32>) {
    let mut s: u64 = 0x9e3779b97f4a7c15;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        (s >> 11) as f32 / (1u64 << 24) as f32 - 0.5
    };
    let mut a = vec![0.0f32; b * NN];
    let mut x = vec![0.0f32; b * N];
    for m in 0..b {
        for r in 0..N {
            let mut acc = 0.0f32;
            for c in 0..N {
                if r != c {
                    let v = rnd();
                    a[m * NN + r * N + c] = v;
                    acc += v.abs();
                }
            }
            a[m * NN + r * N + r] = acc + 1.0;
            x[m * N + r] = rnd();
        }
    }
    (a, x)
}

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    nop: wgpu::ComputePipeline,
    lu: wgpu::ComputePipeline,
    bind: wgpu::BindGroup,
    xbuf: wgpu::Buffer,
    staging: wgpu::Buffer,
    groups: u32,
}

fn wait(device: &wgpu::Device) {
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
}

fn main() {
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        apply_limit_buckets: false,
    })) {
        Ok(a) => a,
        Err(e) => {
            println!("NO GPU ADAPTER: {e:?}");
            return;
        }
    };
    let info = adapter.get_info();
    println!(
        "# adapter: {} ({:?}, {:?})",
        info.name, info.backend, info.device_type
    );
    println!("# driver: {} {}", info.driver, info.driver_info);
    let feats = adapter.features();
    println!(
        "# adapter supports SHADER_F64: {}   (WGSL itself has no f64 type; wgpu's SHADER_F64 is Vulkan/SPIR-V only)",
        feats.contains(wgpu::Features::SHADER_F64)
    );
    let lim = adapter.limits();
    println!(
        "# limits: max_compute_workgroups_per_dimension={} max_storage_buffer_binding_size={} MiB",
        lim.max_compute_workgroups_per_dimension,
        lim.max_storage_buffer_binding_size / (1024 * 1024)
    );

    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("device");

    println!("\n## dispatch and synchronisation latency (batch of 1024 systems, N={N})");
    let g = make_gpu(&device, &queue, 1024);
    let single = time_submit_wait(&g, 1, 1);
    let chain_pass = time_submit_wait(&g, 100, 1);
    let per_submit = time_submit_wait(&g, 1, 100);
    let rt = time_roundtrip(&g, 1024 * N * 4);
    let nop_single = time_pipeline(&g, &g.nop, 1, 1);
    let nop_chain = time_pipeline(&g, &g.nop, 100, 1) / 100.0;
    println!(
        "submit(1 dispatch)+wait                     : {:.1} us",
        single * 1e6
    );
    println!(
        "100 dispatches in one pass, one submit+wait  : {:.2} us per dispatch",
        chain_pass * 1e6 / 100.0
    );
    println!(
        "100 separate submits, one wait               : {:.2} us per submit",
        per_submit * 1e6 / 100.0
    );
    println!(
        "dispatch + copy + map readback ({} KiB)      : {:.1} us round trip",
        1024 * N * 4 / 1024,
        rt * 1e6
    );
    println!(
        "EMPTY kernel: submit+wait                    : {:.1} us",
        nop_single * 1e6
    );
    println!(
        "EMPTY kernel: 100 in one pass                : {:.2} us per dispatch",
        nop_chain * 1e6
    );

    println!("\n## host<->device transfer (write_buffer + submit + wait, then map-read)");
    for floats in [256usize, 1_000, 5_000, 20_000] {
        let (w, r) = time_transfer(&device, &queue, floats);
        println!(
            "{floats:>6} f32 ({:>4} KiB): upload {:.1} us   readback(round trip) {:.1} us",
            floats * 4 / 1024,
            w * 1e6,
            r * 1e6
        );
    }

    println!("\n## batched LU+solve throughput, N={N} (f32 on GPU, f64 on CPU)");
    println!("batch   gpu_us/dispatch  gpu_Msys/s   cpu1_us  cpu1_Msys/s  cpuN_us  cpuN_Msys/s  gpu/cpuN");
    for b in [64usize, 256, 1024, 4096, 16384, 65536] {
        let g = make_gpu(&device, &queue, b);
        // 200 chained dispatches in one pass amortises submit cost.
        let per = time_submit_wait(&g, 200, 1) / 200.0;
        let (c1, cn) = cpu_batch(b);
        println!(
            "{b:<7} {:<16.2} {:<12.2} {:<8.1} {:<12.2} {:<8.1} {:<12.2} {:<8.2}",
            per * 1e6,
            b as f64 / per / 1e6,
            c1 * 1e6,
            b as f64 / c1 / 1e6,
            cn * 1e6,
            b as f64 / cn / 1e6,
            cn / per
        );
    }

    println!("\n## one 30 Hz tick: {SUBSTEPS_PER_TICK} DEPENDENT substeps (budget 33.3 ms)");
    println!("batch   all-in-one-pass_ms  per-substep-readback_ms  cpu_serial_ms");
    for b in [256usize, 1024, 4096] {
        let g = make_gpu(&device, &queue, b);
        let one_pass = time_submit_wait(&g, SUBSTEPS_PER_TICK, 1);
        // Worst case: the CPU must see each substep's result (NR
        // convergence test, probe sampling, device state update).
        let rounds = 200;
        let rt = time_roundtrip_chain(&g, rounds, b * N * 4);
        let (c1, _) = cpu_batch(b);
        println!(
            "{b:<7} {:<19.2} {:<24.1} {:<14.1}",
            one_pass * 1e3,
            rt / rounds as f64 * SUBSTEPS_PER_TICK as f64 * 1e3,
            c1 * SUBSTEPS_PER_TICK as f64 * 1e3
        );
    }
}

fn make_gpu(device: &wgpu::Device, queue: &wgpu::Queue, batch: usize) -> Gpu {
    let (a, x) = diag_dominant_batch(batch);
    let abuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("a"),
        contents: bytemuck::cast_slice(&a),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let xbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("x"),
        contents: bytemuck::cast_slice(&x),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
    });
    let pbuf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("params"),
        contents: bytemuck::bytes_of(&Params {
            count: batch as u32,
            _pad: [0; 3],
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging"),
        size: (batch * N * 4) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("batch_lu"),
        source: wgpu::ShaderSource::Wgsl(include_str!("batch_lu.wgsl").into()),
    });
    // Explicit (shared) layout: auto layouts are exclusive to one pipeline,
    // and both kernels must share one bind group.
    let ro = |read_only: bool| wgpu::BindingType::Buffer {
        ty: wgpu::BufferBindingType::Storage { read_only },
        has_dynamic_offset: false,
        min_binding_size: None,
    };
    let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: ro(true),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 1,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: ro(false),
                count: None,
            },
            wgpu::BindGroupLayoutEntry {
                binding: 2,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let playout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&bgl)],
        immediate_size: 0,
    });
    let mk = |entry: &str| {
        device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some(entry),
            layout: Some(&playout),
            module: &module,
            entry_point: Some(entry),
            compilation_options: Default::default(),
            cache: None,
        })
    };
    let lu = mk("lu_solve");
    let nop = mk("nop");
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: abuf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: xbuf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: pbuf.as_entire_binding(),
            },
        ],
    });
    Gpu {
        device: device.clone(),
        queue: queue.clone(),
        nop,
        lu,
        bind,
        xbuf,
        staging,
        groups: batch.div_ceil(64) as u32,
    }
}

/// `dispatches` LU dispatches per submit, `submits` submits, then one wait.
/// Returns seconds for the whole (dispatches x submits) batch.
fn time_submit_wait(g: &Gpu, dispatches: usize, submits: usize) -> f64 {
    time_pipeline(g, &g.lu, dispatches, submits)
}

/// As `time_submit_wait`, for an arbitrary pipeline (used for the empty
/// kernel, which isolates pure dispatch overhead).
fn time_pipeline(
    g: &Gpu,
    pipeline: &wgpu::ComputePipeline,
    dispatches: usize,
    submits: usize,
) -> f64 {
    let run = || {
        for _ in 0..submits {
            let mut enc = g
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
            {
                let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: None,
                    timestamp_writes: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, &g.bind, &[]);
                for _ in 0..dispatches {
                    pass.dispatch_workgroups(g.groups, 1, 1);
                }
            }
            g.queue.submit(Some(enc.finish()));
        }
        wait(&g.device);
    };
    run();
    let reps = if dispatches * submits > 500 { 3 } else { 20 };
    let t0 = Instant::now();
    for _ in 0..reps {
        run();
    }
    t0.elapsed().as_secs_f64() / reps as f64
}

/// Dispatch, copy the solution to a staging buffer, submit, wait, map,
/// read, unmap: the full CPU round trip.
fn time_roundtrip(g: &Gpu, bytes: usize) -> f64 {
    let once = || {
        let mut enc = g
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&g.lu);
            pass.set_bind_group(0, &g.bind, &[]);
            pass.dispatch_workgroups(g.groups, 1, 1);
        }
        enc.copy_buffer_to_buffer(&g.xbuf, 0, &g.staging, 0, bytes as u64);
        g.queue.submit(Some(enc.finish()));
        let slice = g.staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        wait(&g.device);
        let v = slice.get_mapped_range().expect("map");
        let sum: f32 = bytemuck::cast_slice::<u8, f32>(&v)[0];
        drop(v);
        g.staging.unmap();
        std::hint::black_box(sum);
    };
    once();
    let reps = 200;
    let t0 = Instant::now();
    for _ in 0..reps {
        once();
    }
    t0.elapsed().as_secs_f64() / reps as f64
}

fn time_roundtrip_chain(g: &Gpu, rounds: usize, bytes: usize) -> f64 {
    let t0 = Instant::now();
    for _ in 0..rounds {
        let _ = time_roundtrip_once(g, bytes);
    }
    t0.elapsed().as_secs_f64()
}

fn time_roundtrip_once(g: &Gpu, bytes: usize) -> f32 {
    let mut enc = g
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    {
        let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: None,
            timestamp_writes: None,
        });
        pass.set_pipeline(&g.lu);
        pass.set_bind_group(0, &g.bind, &[]);
        pass.dispatch_workgroups(g.groups, 1, 1);
    }
    enc.copy_buffer_to_buffer(&g.xbuf, 0, &g.staging, 0, bytes as u64);
    g.queue.submit(Some(enc.finish()));
    let slice = g.staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    wait(&g.device);
    let v = slice.get_mapped_range().expect("map");
    let first = bytemuck::cast_slice::<u8, f32>(&v)[0];
    drop(v);
    g.staging.unmap();
    first
}

/// Upload and download timing for a plain f32 vector.
fn time_transfer(device: &wgpu::Device, queue: &wgpu::Queue, floats: usize) -> (f64, f64) {
    let data = vec![1.0f32; floats];
    let bytes = (floats * 4) as u64;
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: bytes,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let up = || {
        queue.write_buffer(&buf, 0, bytemuck::cast_slice(&data));
        queue.submit(None);
        wait(device);
    };
    up();
    let reps = 300;
    let t0 = Instant::now();
    for _ in 0..reps {
        up();
    }
    let tup = t0.elapsed().as_secs_f64() / reps as f64;

    let down = || {
        let mut enc =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(&buf, 0, &staging, 0, bytes);
        queue.submit(Some(enc.finish()));
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        wait(device);
        let v = slice.get_mapped_range().expect("map");
        let f = bytemuck::cast_slice::<u8, f32>(&v)[0];
        drop(v);
        staging.unmap();
        std::hint::black_box(f);
    };
    down();
    let t0 = Instant::now();
    for _ in 0..reps {
        down();
    }
    let tdown = t0.elapsed().as_secs_f64() / reps as f64;
    (tup, tdown)
}

// ------------------------------------------------------------------- CPU side

/// Same batch, f64, on the CPU: (single-thread seconds, all-cores seconds).
fn cpu_batch(b: usize) -> (f64, f64) {
    let (a32, x32) = diag_dominant_batch(b);
    let a: Vec<f64> = a32.iter().map(|v| *v as f64).collect();
    let x: Vec<f64> = x32.iter().map(|v| *v as f64).collect();

    let one = |reps: usize| -> f64 {
        let mut xs = x.clone();
        let mut lu = sim_math::DenseLu::new(N);
        let t0 = Instant::now();
        for _ in 0..reps {
            for m in 0..b {
                lu.factor(&a[m * NN..(m + 1) * NN]);
                lu.solve(&mut xs[m * N..(m + 1) * N]);
            }
        }
        t0.elapsed().as_secs_f64() / reps as f64
    };
    let reps = (2_000_000 / b).max(3);
    one(reps);
    let c1 = one(reps);

    use rayon::prelude::*;
    let par = |reps: usize| -> f64 {
        let mut xs = x.clone();
        let t0 = Instant::now();
        for _ in 0..reps {
            xs.par_chunks_mut(N * 64)
                .enumerate()
                .for_each(|(ci, chunk)| {
                    let mut lu = sim_math::DenseLu::new(N);
                    let base = ci * 64;
                    for (j, xj) in chunk.chunks_mut(N).enumerate() {
                        let m = base + j;
                        lu.factor(&a[m * NN..(m + 1) * NN]);
                        lu.solve(xj);
                    }
                });
        }
        t0.elapsed().as_secs_f64() / reps as f64
    };
    par(reps.min(50));
    let cn = par(reps.min(50));
    (c1, cn)
}
