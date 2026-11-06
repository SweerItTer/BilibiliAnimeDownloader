//! Opt-in timings from the real native window; no capture or alternate renderer.

#[derive(Default)]
pub struct ScrollTimings {
    enabled: bool,
    previous: Option<(f64, bool)>,
    cpu_ms: Vec<f64>,
    frame_ms: Vec<f64>,
}

pub fn enabled() -> bool {
    std::env::var("BILI_GUI_RENDER_DIAGNOSTICS").as_deref() == Ok("1")
}

impl ScrollTimings {
    pub fn from_environment() -> Self {
        Self {
            enabled: enabled(),
            ..Self::default()
        }
    }

    pub fn observe(&mut self, ctx: &egui::Context, frame: &eframe::Frame) {
        if !self.enabled {
            return;
        }
        let (time, scrolling) = ctx.input(|input| {
            (
                input.time,
                input.smooth_scroll_delta != egui::Vec2::ZERO
                    || input.raw_scroll_delta != egui::Vec2::ZERO,
            )
        });
        if let Some((previous_time, true)) = self.previous {
            if let Some(cpu) = frame.info().cpu_usage {
                if self.cpu_ms.len() < 512 {
                    self.cpu_ms.push(f64::from(cpu) * 1000.0);
                }
            }
            if scrolling && self.frame_ms.len() < 512 {
                self.frame_ms.push((time - previous_time) * 1000.0);
            }
            if !scrolling {
                if self.cpu_ms.len() >= 3 {
                    eprintln!(
                        "render_scroll samples={} cpu_p95_ms={:.2} interval_p95_ms={:.2}",
                        self.cpu_ms.len(),
                        percentile(&mut self.cpu_ms),
                        percentile(&mut self.frame_ms)
                    );
                }
                self.cpu_ms.clear();
                self.frame_ms.clear();
            }
        }
        self.previous = Some((time, scrolling));
    }
}

fn percentile(samples: &mut [f64]) -> f64 {
    samples.sort_by(f64::total_cmp);
    samples
        .get(samples.len().saturating_sub(1) * 95 / 100)
        .copied()
        .unwrap_or(0.0)
}

pub fn report_renderer(cc: &eframe::CreationContext<'_>) {
    if !enabled() {
        return;
    }
    if let Some(gl) = &cc.gl {
        use eframe::glow::HasContext;
        // Only public driver strings; never URLs, UI text or account information.
        unsafe {
            eprintln!(
                "render_driver vendor={} renderer={} version={}",
                gl.get_parameter_string(eframe::glow::VENDOR),
                gl.get_parameter_string(eframe::glow::RENDERER),
                gl.get_parameter_string(eframe::glow::VERSION)
            );
        }
    }
}
