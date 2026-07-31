// Batched dense LU + triangular solve: one GPU invocation per system.
// N is the per-system matrix order. f32 only: WGSL has no f64 type, and
// wgpu's SHADER_F64 feature is Vulkan/SPIR-V-only, so this is the best
// precision a portable GPU path can offer.

const N: u32 = 8u;
const NN: u32 = 64u; // N*N

struct Params {
    count: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(0) @binding(0) var<storage, read> a: array<f32>;
@group(0) @binding(1) var<storage, read_write> x: array<f32>;
@group(0) @binding(2) var<uniform> p: Params;

// Empty kernel: measures pure dispatch cost.
@compute @workgroup_size(64)
fn nop(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x == 0xffffffffu) {
        x[0] = 0.0;
    }
}

// One system per invocation. Reads the RHS from x and writes the solution
// back to x, so chained dispatches are genuinely data-dependent — the same
// dependency Newton-Raphson and the fixed timestep impose.
@compute @workgroup_size(64)
fn lu_solve(@builtin(global_invocation_id) gid: vec3<u32>) {
    let b = gid.x;
    if (b >= p.count) {
        return;
    }
    var m: array<f32, 64>;
    let ab = b * NN;
    for (var i = 0u; i < NN; i = i + 1u) {
        m[i] = a[ab + i];
    }
    var v: array<f32, 8>;
    let xb = b * N;
    for (var i = 0u; i < N; i = i + 1u) {
        v[i] = x[xb + i];
    }
    // Unpivoted LU (the matrices are diagonally dominant by construction).
    for (var k = 0u; k < N; k = k + 1u) {
        let piv = m[k * N + k];
        for (var r = k + 1u; r < N; r = r + 1u) {
            let f = m[r * N + k] / piv;
            m[r * N + k] = f;
            for (var c = k + 1u; c < N; c = c + 1u) {
                m[r * N + c] = m[r * N + c] - f * m[k * N + c];
            }
        }
    }
    // Forward substitution (unit diagonal).
    for (var k = 0u; k < N; k = k + 1u) {
        for (var r = k + 1u; r < N; r = r + 1u) {
            v[r] = v[r] - m[r * N + k] * v[k];
        }
    }
    // Back substitution.
    for (var k = N; k > 0u; k = k - 1u) {
        let i = k - 1u;
        var s = v[i];
        for (var c = i + 1u; c < N; c = c + 1u) {
            s = s - m[i * N + c] * v[c];
        }
        v[i] = s / m[i * N + i];
    }
    for (var i = 0u; i < N; i = i + 1u) {
        x[xb + i] = v[i];
    }
}
