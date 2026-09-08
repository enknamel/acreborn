//! Frame cost accounting for `--perf`: per-frame wall time and what the
//! GPU pass drew, summarised at exit. The status line shows the live
//! numbers; the headless loop renders offscreen at the frame cap so a
//! `--connect --screenshot --perf` run measures what a window would.

use std::time::Instant;

use crate::gpu::{FrameStats, Gpu};

#[derive(Default)]
pub struct Perf {
    /// Wall time of each rendered frame, ms (tick, overlay, encode and,
    /// headlessly, the GPU finishing it).
    frame_ms: Vec<f32>,
    /// Time from the start of the frame to the end of the CPU work
    /// (everything but waiting for the GPU), ms.
    cpu_ms: Vec<f32>,
    /// Of that, the egui pass (`run_overlay`), ms.
    overlay_ms: Vec<f32>,
    encode_ms: Vec<f32>,
    draw_calls: u64,
    triangles: u64,
    instances: u64,
    instances_culled: u64,
    batches: u64,
    batches_culled: u64,
    particles: u64,
    /// Frames the window skipped because nothing had changed.
    pub idle_frames: u64,
    /// The stats of the last rendered frame, for the status line.
    pub last: FrameStats,
    pub last_frame_ms: f32,
}

impl Perf {
    /// Record a rendered frame.
    pub fn frame(&mut self, frame_ms: f32, cpu_ms: f32, overlay_ms: f32, stats: FrameStats) {
        self.frame_ms.push(frame_ms);
        self.cpu_ms.push(cpu_ms);
        self.overlay_ms.push(overlay_ms);
        self.encode_ms.push(stats.encode_ms);
        self.draw_calls += stats.draw_calls as u64;
        self.triangles += stats.triangles;
        self.instances += stats.instances as u64;
        self.instances_culled += stats.instances_culled as u64;
        self.batches += stats.batches as u64;
        self.batches_culled += stats.batches_culled as u64;
        self.particles += stats.particles as u64;
        self.last = stats;
        self.last_frame_ms = frame_ms;
    }

    /// The status line's part: last frame's cost and draw counts.
    pub fn status(&self) -> String {
        let s = &self.last;
        format!(
            "  {:.1} ms  draws {}  tris {}k  inst {}/{}  batches {}/{}",
            self.last_frame_ms,
            s.draw_calls,
            s.triangles / 1000,
            s.instances,
            s.instances + s.instances_culled,
            s.batches,
            s.batches + s.batches_culled,
        )
    }

    /// The exit report: averages and percentiles over every rendered
    /// frame, plus what the GPU holds now.
    pub fn summary(&self, gpu: &Gpu, gpu_meshes: usize, since: Instant) -> String {
        let n = self.frame_ms.len().max(1) as f64;
        let pct = |v: &[f32], p: f64| -> f32 {
            if v.is_empty() {
                return 0.0;
            }
            let mut s = v.to_vec();
            s.sort_by(|a, b| a.total_cmp(b));
            s[((s.len() - 1) as f64 * p).round() as usize]
        };
        let avg = |v: &[f32]| v.iter().map(|x| *x as f64).sum::<f64>() / v.len().max(1) as f64;
        let (buffers, buffer_bytes) = gpu.buffer_stats();
        let mut out = String::new();
        out += &format!(
            "perf: {} frames rendered, {} idle, over {:.1} s\n",
            self.frame_ms.len(),
            self.idle_frames,
            since.elapsed().as_secs_f64()
        );
        out += &format!(
            "  frame ms: avg {:.2}  p50 {:.2}  p95 {:.2}  max {:.2}  (cpu avg {:.2} of which overlay {:.2}; encode avg {:.2})\n",
            avg(&self.frame_ms),
            pct(&self.frame_ms, 0.5),
            pct(&self.frame_ms, 0.95),
            pct(&self.frame_ms, 1.0),
            avg(&self.cpu_ms),
            avg(&self.overlay_ms),
            avg(&self.encode_ms),
        );
        out += &format!(
            "  per frame: {:.0} draw calls, {:.0}k triangles, {:.0} instances drawn ({:.0} culled), {:.0} batches drawn ({:.0} culled), {:.0} particles\n",
            self.draw_calls as f64 / n,
            self.triangles as f64 / n / 1000.0,
            self.instances as f64 / n,
            self.instances_culled as f64 / n,
            self.batches as f64 / n,
            self.batches_culled as f64 / n,
            self.particles as f64 / n,
        );
        out += &format!(
            "  gpu: {} materials ({} MB textures), {} gpu meshes, {} static batches, {} buffers created ({} MB)",
            gpu.material_count(),
            gpu.texture_bytes() >> 20,
            gpu_meshes,
            gpu.batch_count(),
            buffers,
            buffer_bytes >> 20,
        );
        out
    }
}
