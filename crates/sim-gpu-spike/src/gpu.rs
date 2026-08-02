//! wgpu/WGSL batched dense LU with partial pivoting.
//!
//! One workgroup = one matrix (the design's island-batching model). The
//! matrix and RHS live in workgroup shared memory; pivot search and the
//! triangular solves run serially on thread 0 (n ≤ 64 — there is no
//! parallelism worth harvesting there), row elimination fans across the
//! workgroup. Factors + pivots are persisted to storage buffers so a
//! second `solve_only` entry point can run iterative-refinement solves
//! against fresh RHS vectors without refactoring.
//!
//! Two precision variants:
//! - f32: native. Metal/WGSL has no f64 — this is the hard platform limit.
//! - df64 (double-single, hi+lo f32 pair): ~2×24-bit mantissa via
//!   two_sum/two_prod (Dekker/Knuth; two_prod leans on fma()).
//!
//! No determinism claim of any kind is made for either variant.

use std::borrow::Cow;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Precision {
    F32,
    Df64,
}

impl Precision {
    pub fn elem_bytes(self) -> usize {
        match self {
            Precision::F32 => 4,
            Precision::Df64 => 8,
        }
    }
    /// Workgroup shared memory needed for padded tile + rhs.
    pub fn shared_bytes(self, n: usize) -> usize {
        self.elem_bytes() * (n * (n + 1) + n) + 8
    }
}

pub struct Ctx {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    pub max_wg_storage: u32,
}

impl Ctx {
    pub fn new() -> Ctx {
        pollster::block_on(async {
            let instance = wgpu::Instance::default();
            let adapter = instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::HighPerformance,
                    ..Default::default()
                })
                .await
                .expect("no GPU adapter available");
            let info = adapter.get_info();
            let alims = adapter.limits();
            let mut limits = wgpu::Limits::default();
            // Default limit is 16 KiB; ask for what the hardware has so the
            // n=64 f32 tile (16.6 KiB) fits.
            limits.max_compute_workgroup_storage_size = alims.max_compute_workgroup_storage_size;
            let (device, queue) = adapter
                .request_device(
                    &wgpu::DeviceDescriptor {
                        label: Some("sim-gpu-spike"),
                        required_features: wgpu::Features::empty(),
                        required_limits: limits,
                        memory_hints: wgpu::MemoryHints::Performance,
                    },
                    None,
                )
                .await
                .expect("request_device failed");
            Ctx {
                device,
                queue,
                info,
                max_wg_storage: alims.max_compute_workgroup_storage_size,
            }
        })
    }
}

const SHADER_F32: &str = r#"
// Batched dense LU + solve, one workgroup per matrix, f32.
@group(0) @binding(0) var<storage, read> a_in: array<f32>;
@group(0) @binding(1) var<storage, read> b_in: array<f32>;
@group(0) @binding(2) var<storage, read_write> x_out: array<f32>;
@group(0) @binding(3) var<storage, read_write> lu_out: array<f32>;
@group(0) @binding(4) var<storage, read_write> piv_out: array<u32>;
@group(0) @binding(5) var<storage, read_write> flags: array<u32>;

const N: u32 = @N@u;
const NN: u32 = @NN@u;
// Padded row stride: N+1 so column accesses don't hit the same bank.
const NS: u32 = @NS@u;
const TS: u32 = @TS@u;
const WG: u32 = @WG@u;
const PIVOT_TOL: f32 = 1e-30;

var<workgroup> tile: array<f32, TS>;
var<workgroup> rhs: array<f32, N>;
var<workgroup> piv_k: u32;
var<workgroup> sing: u32;

@compute @workgroup_size(@WG@)
fn factor_solve(@builtin(workgroup_id) wid: vec3<u32>,
                @builtin(local_invocation_index) t: u32) {
    let m = wid.x;
    for (var i = t; i < NN; i += WG) { tile[(i / N) * NS + (i % N)] = a_in[m * NN + i]; }
    for (var i = t; i < N; i += WG) { rhs[i] = b_in[m * N + i]; }
    if (t == 0u) { sing = 0u; }
    workgroupBarrier();

    for (var k = 0u; k < N; k += 1u) {
        if (t == 0u) {
            var p = k;
            var pmax = abs(tile[k * NS + k]);
            for (var r = k + 1u; r < N; r += 1u) {
                let v = abs(tile[r * NS + k]);
                if (v > pmax) { pmax = v; p = r; }
            }
            piv_k = p;
            piv_out[m * N + k] = p;
            if (pmax < PIVOT_TOL) { sing = 1u; }
        }
        workgroupBarrier();
        let p = piv_k;
        if (p != k) {
            for (var c = t; c < N; c += WG) {
                let tmp = tile[k * NS + c];
                tile[k * NS + c] = tile[p * NS + c];
                tile[p * NS + c] = tmp;
            }
            if (t == 0u) {
                let tb = rhs[k]; rhs[k] = rhs[p]; rhs[p] = tb;
            }
        }
        workgroupBarrier();
        let pivot = tile[k * NS + k];
        for (var r = k + 1u + t; r < N; r += WG) {
            let mf = tile[r * NS + k] / pivot;
            tile[r * NS + k] = mf;
            for (var c = k + 1u; c < N; c += 1u) {
                tile[r * NS + c] = tile[r * NS + c] - mf * tile[k * NS + c];
            }
        }
        workgroupBarrier();
    }

    if (t == 0u) {
        flags[m] = sing;
        // Forward-substitute L (unit diagonal); pivots already applied to rhs.
        for (var k = 0u; k < N; k += 1u) {
            let xk = rhs[k];
            for (var r = k + 1u; r < N; r += 1u) {
                rhs[r] = rhs[r] - tile[r * NS + k] * xk;
            }
        }
        // Back-substitute U.
        var k2: i32 = i32(N) - 1;
        while (k2 >= 0) {
            let ku = u32(k2);
            var s = rhs[ku];
            for (var c = ku + 1u; c < N; c += 1u) {
                s = s - tile[ku * NS + c] * rhs[c];
            }
            rhs[ku] = s / tile[ku * NS + ku];
            k2 = k2 - 1;
        }
    }
    workgroupBarrier();
    for (var i = t; i < N; i += WG) { x_out[m * N + i] = rhs[i]; }
    for (var i = t; i < NN; i += WG) { lu_out[m * NN + i] = tile[(i / N) * NS + (i % N)]; }
}

// Solve with previously stored factors against a fresh b_in (iterative
// refinement: the residual is computed in f64 on the CPU, rounded to f32,
// and solved here without refactoring).
@compute @workgroup_size(@WG@)
fn solve_only(@builtin(workgroup_id) wid: vec3<u32>,
              @builtin(local_invocation_index) t: u32) {
    let m = wid.x;
    for (var i = t; i < NN; i += WG) { tile[(i / N) * NS + (i % N)] = lu_out[m * NN + i]; }
    for (var i = t; i < N; i += WG) { rhs[i] = b_in[m * N + i]; }
    workgroupBarrier();
    if (t == 0u) {
        for (var k = 0u; k < N; k += 1u) {
            let p = piv_out[m * N + k];
            if (p != k) { let tb = rhs[k]; rhs[k] = rhs[p]; rhs[p] = tb; }
        }
        for (var k = 0u; k < N; k += 1u) {
            let xk = rhs[k];
            for (var r = k + 1u; r < N; r += 1u) {
                rhs[r] = rhs[r] - tile[r * NS + k] * xk;
            }
        }
        var k2: i32 = i32(N) - 1;
        while (k2 >= 0) {
            let ku = u32(k2);
            var s = rhs[ku];
            for (var c = ku + 1u; c < N; c += 1u) {
                s = s - tile[ku * NS + c] * rhs[c];
            }
            rhs[ku] = s / tile[ku * NS + ku];
            k2 = k2 - 1;
        }
    }
    workgroupBarrier();
    for (var i = t; i < N; i += WG) { x_out[m * N + i] = rhs[i]; }
}
"#;

const SHADER_DF64: &str = r#"
// Batched dense LU + solve in double-single ("df64") arithmetic:
// each value is vec2<f32>(hi, lo). two_prod relies on fma() being fused;
// if the backend contracts it away the error term collapses to 0 and
// df64 silently degrades toward f32 — the CPU-side f64 residual check
// in the harness is what detects that, by design.
@group(0) @binding(0) var<storage, read> a_in: array<vec2<f32>>;
@group(0) @binding(1) var<storage, read> b_in: array<vec2<f32>>;
@group(0) @binding(2) var<storage, read_write> x_out: array<vec2<f32>>;
@group(0) @binding(3) var<storage, read_write> lu_out: array<vec2<f32>>;
@group(0) @binding(4) var<storage, read_write> piv_out: array<u32>;
@group(0) @binding(5) var<storage, read_write> flags: array<u32>;

const N: u32 = @N@u;
const NN: u32 = @NN@u;
// Padded row stride: N+1 so column accesses don't hit the same bank.
const NS: u32 = @NS@u;
const TS: u32 = @TS@u;
const WG: u32 = @WG@u;
const PIVOT_TOL: f32 = 1e-30;

var<workgroup> tile: array<vec2<f32>, TS>;
var<workgroup> rhs: array<vec2<f32>, N>;
var<workgroup> piv_k: u32;
var<workgroup> sing: u32;

fn qts(a: f32, b: f32) -> vec2<f32> {
    let s = a + b;
    return vec2<f32>(s, b - (s - a));
}
fn two_sum(a: f32, b: f32) -> vec2<f32> {
    let s = a + b;
    let bb = s - a;
    return vec2<f32>(s, (a - (s - bb)) + (b - bb));
}
fn two_prod(a: f32, b: f32) -> vec2<f32> {
    let p = a * b;
    return vec2<f32>(p, fma(a, b, -p));
}
fn df_add(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let s = two_sum(a.x, b.x);
    return qts(s.x, s.y + (a.y + b.y));
}
fn df_sub(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    return df_add(a, vec2<f32>(-b.x, -b.y));
}
fn df_mul(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let p = two_prod(a.x, b.x);
    return qts(p.x, p.y + (a.x * b.y + a.y * b.x));
}
fn df_div(a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let q1 = a.x / b.x;
    let r1 = df_sub(a, df_mul(vec2<f32>(q1, 0.0), b));
    let q2 = r1.x / b.x;
    let r2 = df_sub(r1, df_mul(vec2<f32>(q2, 0.0), b));
    let q3 = r2.x / b.x;
    return df_add(qts(q1, q2), vec2<f32>(q3, 0.0));
}

@compute @workgroup_size(@WG@)
fn factor_solve(@builtin(workgroup_id) wid: vec3<u32>,
                @builtin(local_invocation_index) t: u32) {
    let m = wid.x;
    for (var i = t; i < NN; i += WG) { tile[(i / N) * NS + (i % N)] = a_in[m * NN + i]; }
    for (var i = t; i < N; i += WG) { rhs[i] = b_in[m * N + i]; }
    if (t == 0u) { sing = 0u; }
    workgroupBarrier();

    for (var k = 0u; k < N; k += 1u) {
        if (t == 0u) {
            var p = k;
            var pmax = abs(tile[k * NS + k].x);
            for (var r = k + 1u; r < N; r += 1u) {
                let v = abs(tile[r * NS + k].x);
                if (v > pmax) { pmax = v; p = r; }
            }
            piv_k = p;
            piv_out[m * N + k] = p;
            if (pmax < PIVOT_TOL) { sing = 1u; }
        }
        workgroupBarrier();
        let p = piv_k;
        if (p != k) {
            for (var c = t; c < N; c += WG) {
                let tmp = tile[k * NS + c];
                tile[k * NS + c] = tile[p * NS + c];
                tile[p * NS + c] = tmp;
            }
            if (t == 0u) {
                let tb = rhs[k]; rhs[k] = rhs[p]; rhs[p] = tb;
            }
        }
        workgroupBarrier();
        let pivot = tile[k * NS + k];
        for (var r = k + 1u + t; r < N; r += WG) {
            let mf = df_div(tile[r * NS + k], pivot);
            tile[r * NS + k] = mf;
            for (var c = k + 1u; c < N; c += 1u) {
                tile[r * NS + c] = df_sub(tile[r * NS + c], df_mul(mf, tile[k * NS + c]));
            }
        }
        workgroupBarrier();
    }

    if (t == 0u) {
        flags[m] = sing;
        for (var k = 0u; k < N; k += 1u) {
            let xk = rhs[k];
            for (var r = k + 1u; r < N; r += 1u) {
                rhs[r] = df_sub(rhs[r], df_mul(tile[r * NS + k], xk));
            }
        }
        var k2: i32 = i32(N) - 1;
        while (k2 >= 0) {
            let ku = u32(k2);
            var s = rhs[ku];
            for (var c = ku + 1u; c < N; c += 1u) {
                s = df_sub(s, df_mul(tile[ku * NS + c], rhs[c]));
            }
            rhs[ku] = df_div(s, tile[ku * NS + ku]);
            k2 = k2 - 1;
        }
    }
    workgroupBarrier();
    for (var i = t; i < N; i += WG) { x_out[m * N + i] = rhs[i]; }
    for (var i = t; i < NN; i += WG) { lu_out[m * NN + i] = tile[(i / N) * NS + (i % N)]; }
}
"#;

/// A persistent batch solver: buffers + pipelines for one (n, batch,
/// precision) configuration. Everything is GPU-resident; per-call traffic
/// is only dispatch + (optional) x readback — the Isaac Gym pattern.
pub struct GpuBatch {
    device: wgpu::Device,
    queue: wgpu::Queue,
    n: usize,
    batch: usize,
    precision: Precision,
    pipeline_lu: wgpu::ComputePipeline,
    pipeline_solve: Option<wgpu::ComputePipeline>,
    bind_group: wgpu::BindGroup,
    a_in: wgpu::Buffer,
    b_in: wgpu::Buffer,
    x_out: wgpu::Buffer,
    staging: wgpu::Buffer,
}

impl GpuBatch {
    /// Returns None if the tile would not fit in workgroup shared memory.
    pub fn new(ctx: &Ctx, n: usize, batch: usize, precision: Precision) -> Option<GpuBatch> {
        if precision.shared_bytes(n) > ctx.max_wg_storage as usize {
            return None;
        }
        let device = &ctx.device;
        let wg: usize = if n <= 32 { 32 } else { 64 };
        let template = match precision {
            Precision::F32 => SHADER_F32,
            Precision::Df64 => SHADER_DF64,
        };
        let src = template
            .replace("@NN@", &(n * n).to_string())
            .replace("@NS@", &(n + 1).to_string())
            .replace("@TS@", &(n * (n + 1)).to_string())
            .replace("@N@", &n.to_string())
            .replace("@WG@", &wg.to_string());
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lu"),
            source: wgpu::ShaderSource::Wgsl(Cow::Owned(src)),
        });

        let storage = |binding, read_only| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                storage(0, true),
                storage(1, true),
                storage(2, false),
                storage(3, false),
                storage(4, false),
                storage(5, false),
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[&bgl],
            push_constant_ranges: &[],
        });
        let mk_pipeline = |entry: &str| {
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: wgpu::PipelineCompilationOptions::default(),
                cache: None,
            })
        };
        let pipeline_lu = mk_pipeline("factor_solve");
        let pipeline_solve = match precision {
            Precision::F32 => Some(mk_pipeline("solve_only")),
            Precision::Df64 => None,
        };

        let eb = precision.elem_bytes() as u64;
        let buf = |label: &str, size: u64, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size.max(4),
                usage,
                mapped_at_creation: false,
            })
        };
        use wgpu::BufferUsages as U;
        let nn = (n * n * batch) as u64;
        let nb = (n * batch) as u64;
        let a_in = buf("a_in", nn * eb, U::STORAGE | U::COPY_DST);
        let b_in = buf("b_in", nb * eb, U::STORAGE | U::COPY_DST);
        let x_out = buf("x_out", nb * eb, U::STORAGE | U::COPY_SRC);
        let lu_out = buf("lu_out", nn * eb, U::STORAGE);
        let piv_out = buf("piv_out", nb * 4, U::STORAGE);
        let flags = buf("flags", batch as u64 * 4, U::STORAGE | U::COPY_SRC);
        let staging = buf("staging", nb * eb, U::MAP_READ | U::COPY_DST);

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &bgl,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: a_in.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: b_in.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: x_out.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: lu_out.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: piv_out.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: flags.as_entire_binding() },
            ],
        });

        Some(GpuBatch {
            device: ctx.device.clone(),
            queue: ctx.queue.clone(),
            n,
            batch,
            precision,
            pipeline_lu,
            pipeline_solve,
            bind_group,
            a_in,
            b_in,
            x_out,
            staging,
        })
    }

    fn encode(&self, values: &[f64]) -> Vec<u8> {
        match self.precision {
            Precision::F32 => {
                let v: Vec<f32> = values.iter().map(|&x| x as f32).collect();
                bytemuck::cast_slice(&v).to_vec()
            }
            Precision::Df64 => {
                let mut v = Vec::with_capacity(values.len() * 2);
                for &x in values {
                    let hi = x as f32;
                    let lo = (x - hi as f64) as f32;
                    v.push(hi);
                    v.push(lo);
                }
                bytemuck::cast_slice(&v).to_vec()
            }
        }
    }

    pub fn upload(&self, a: &[f64], b: &[f64]) {
        self.queue.write_buffer(&self.a_in, 0, &self.encode(a));
        self.queue.write_buffer(&self.b_in, 0, &self.encode(b));
    }

    /// Overwrite the RHS buffer only (iterative refinement).
    pub fn upload_rhs(&self, r: &[f64]) {
        self.queue.write_buffer(&self.b_in, 0, &self.encode(r));
    }

    fn dispatch(&self, pipeline: &wgpu::ComputePipeline) {
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind_group, &[]);
            pass.dispatch_workgroups(self.batch as u32, 1, 1);
        }
        self.queue.submit(Some(enc.finish()));
    }

    /// Submit one factor+solve pass over the whole batch (no wait).
    pub fn factor_solve(&self) {
        self.dispatch(&self.pipeline_lu);
    }

    /// Solve-only against stored factors (no wait).
    pub fn solve_only(&self) {
        self.dispatch(self.pipeline_solve.as_ref().expect("f32 only"));
    }

    pub fn wait(&self) {
        self.device.poll(wgpu::Maintain::Wait);
    }

    /// Copy x back to host, blocking. Returns f64-widened solutions.
    pub fn read_x(&self) -> Vec<f64> {
        let bytes = (self.n * self.batch * self.precision.elem_bytes()) as u64;
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(&self.x_out, 0, &self.staging, 0, bytes);
        self.queue.submit(Some(enc.finish()));
        let slice = self.staging.slice(..bytes);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |res| {
            tx.send(res).unwrap();
        });
        self.device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().expect("map_async failed");
        let out = {
            let data = slice.get_mapped_range();
            let f: &[f32] = bytemuck::cast_slice(&data);
            match self.precision {
                Precision::F32 => f.iter().map(|&x| x as f64).collect::<Vec<f64>>(),
                Precision::Df64 => f
                    .chunks_exact(2)
                    .map(|p| p[0] as f64 + p[1] as f64)
                    .collect::<Vec<f64>>(),
            }
        };
        self.staging.unmap();
        out
    }
}
