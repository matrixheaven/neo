//! Bounded per-process debug logging for terminal rendering.

use std::{
    collections::VecDeque,
    env,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Mutex, OnceLock},
};

use crate::primitive::visible_width;

use super::frame_differ::{DiffRender, RenderDimensions, TuiRenderer};

const DEBUG_LOG_CAPACITY: usize = 1024 * 1024;
static NEXT_FRAME_ID: AtomicU64 = AtomicU64::new(1);
static NEXT_CRASH_ID: AtomicU64 = AtomicU64::new(1);
static PROCESS_LOGGER: OnceLock<Mutex<Option<DebugFrameLogger>>> = OnceLock::new();

pub(super) struct DebugFrameLogger {
    path: PathBuf,
    capacity: usize,
    records: VecDeque<Vec<u8>>,
    total_len: usize,
}

impl DebugFrameLogger {
    fn new(directory: &Path, capacity: usize) -> std::io::Result<Self> {
        fs::create_dir_all(directory)?;
        let path = directory.join(format!("frames-{}.log", std::process::id()));
        fs::File::create(&path)?;
        Ok(Self {
            path,
            capacity: capacity.max(1),
            records: VecDeque::new(),
            total_len: 0,
        })
    }

    fn record_frame(&mut self, frame_id: u64, phase: &str, body: &str) -> std::io::Result<()> {
        let mut record = format!("frame={frame_id} phase={phase}\n").into_bytes();
        if record.len() >= self.capacity {
            record.truncate(self.capacity);
        } else {
            let body_capacity = self.capacity - record.len() - 1;
            let mut body_start = body.len().saturating_sub(body_capacity);
            while body_start < body.len() && !body.is_char_boundary(body_start) {
                body_start += 1;
            }
            record.extend_from_slice(&body.as_bytes()[body_start..]);
        }
        record.push(b'\n');
        record.truncate(self.capacity);
        while self.total_len.saturating_add(record.len()) > self.capacity {
            let Some(removed) = self.records.pop_front() else {
                break;
            };
            self.total_len = self.total_len.saturating_sub(removed.len());
        }
        self.total_len = self.total_len.saturating_add(record.len());
        self.records.push_back(record);
        self.rewrite()
    }

    fn rewrite(&self) -> std::io::Result<()> {
        let mut file = fs::File::create(&self.path)?;
        for record in &self.records {
            file.write_all(record)?;
        }
        file.flush()
    }

    #[cfg(test)]
    fn path(&self) -> &Path {
        &self.path
    }
}

pub(super) fn debug_log_enabled() -> bool {
    env::var("NEO_TUI_DEBUG").is_ok_and(|value| value == "1")
}

fn next_frame_id() -> u64 {
    NEXT_FRAME_ID.fetch_add(1, Ordering::Relaxed)
}

fn record_process_frame(frame_id: u64, phase: &str, body: &str) -> std::io::Result<()> {
    let logger = PROCESS_LOGGER.get_or_init(|| {
        let directory = env::temp_dir().join("neo-tui-debug");
        Mutex::new(DebugFrameLogger::new(&directory, DEBUG_LOG_CAPACITY).ok())
    });
    let mut logger = logger
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let Some(logger) = logger.as_mut() else {
        return Ok(());
    };
    logger.record_frame(frame_id, phase, body)
}

pub(super) fn write_output_log(frame_id: u64, label: &str, buffer: &str) -> std::io::Result<()> {
    record_process_frame(frame_id, &format!("{label}-output"), buffer)
}

pub(super) fn write_debug_log(
    frame_id: u64,
    label: &str,
    width: u16,
    height: usize,
    new_lines: &[String],
    previous_lines: &[String],
    extra: Option<&str>,
) -> std::io::Result<()> {
    let mut body = format!(
        "width={width} height={height}\nnew_lines.len()={} previous_lines.len()={}\n",
        new_lines.len(),
        previous_lines.len()
    );
    if let Some(extra) = extra {
        let _ = writeln!(body, "{extra}");
    }
    write_rendered_lines(&mut body, "=== new_lines ===", new_lines);
    write_rendered_lines(&mut body, "=== previous_lines ===", previous_lines);
    record_process_frame(frame_id, label, &body)
}

fn write_rendered_lines(output: &mut String, heading: &str, lines: &[String]) {
    let _ = writeln!(output, "{heading}");
    for (index, line) in lines.iter().enumerate() {
        let width = visible_width(line);
        let _ = writeln!(output, "[{index}] (w={width}) {line}");
    }
}

fn write_width_crash_log(
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<PathBuf> {
    let directory = env::temp_dir().join("neo-tui-debug");
    fs::create_dir_all(&directory)?;
    let (path, mut file) = loop {
        let crash_id = NEXT_CRASH_ID.fetch_add(1, Ordering::Relaxed);
        let path = directory.join(format!("width-crash-{}-{crash_id}.log", std::process::id()));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => break (path, file),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    };
    writeln!(file, "Terminal width: {width}")?;
    writeln!(file, "Offending line index: {offender_idx}")?;
    writeln!(
        file,
        "Offending line visible width: {}",
        visible_width(&new_lines[offender_idx])
    )?;
    let mut rendered = String::new();
    write_rendered_lines(&mut rendered, "=== All rendered lines ===", new_lines);
    file.write_all(rendered.as_bytes())?;
    file.flush()?;
    Ok(path)
}

pub(super) fn check_line_widths(width: u16, new_lines: &[String]) -> std::io::Result<()> {
    for (index, line) in new_lines.iter().enumerate() {
        let line_width = visible_width(line);
        if line_width > usize::from(width) {
            let path = write_width_crash_log(width, new_lines, index)?;
            return Err(std::io::Error::other(format!(
                "rendered line {index} exceeds terminal width ({line_width} > {width}). crash log: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

impl TuiRenderer {
    pub(super) fn log_render_start(&mut self, dimensions: RenderDimensions, new_lines: &[String]) {
        if !debug_log_enabled() {
            return;
        }
        self.debug_frame_id = next_frame_id();
        let _ = write_debug_log(
            self.debug_frame_id,
            "render-start",
            dimensions.width,
            dimensions.height,
            new_lines,
            &self.previous_lines,
            Some(&format!(
                "previous_width={} previous_height={} previous_viewport_top={} viewport_top={} hardware_cursor_row={} first_render={} clear_on_shrink={}",
                self.previous_width,
                self.previous_height,
                self.previous_viewport_top,
                self.viewport_top,
                self.hardware_cursor_row,
                self.first_render,
                self.clear_on_shrink
            )),
        );
    }

    pub(super) fn log_diff_render(
        &self,
        dimensions: RenderDimensions,
        new_lines: &[String],
        diff_render: &DiffRender,
    ) {
        if !debug_log_enabled() {
            return;
        }
        let _ = write_output_log(self.debug_frame_id, "diff", &diff_render.buffer);
        let _ = write_debug_log(
            self.debug_frame_id,
            "diff-render",
            dimensions.width,
            dimensions.height,
            new_lines,
            &self.previous_lines,
            Some(&format!(
                "first_changed={} last_changed={} append_start={} prev_viewport_top={} viewport_top={} hardware_cursor_row={} move_target_row={} render_end={} final_cursor_row={}",
                diff_render.change_range.first,
                diff_render.change_range.last,
                diff_render.change_range.append_start,
                diff_render.viewport.previous_top,
                diff_render.viewport.top,
                diff_render.viewport.hardware_cursor_row,
                diff_render.move_target_row,
                diff_render.render_end,
                diff_render.final_cursor_row
            )),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_logger_uses_one_bounded_file_and_unique_frame_ids() {
        let dir = tempfile::tempdir().unwrap();
        let mut logger = DebugFrameLogger::new(dir.path(), 1024).unwrap();
        let first_frame = next_frame_id();
        let second_frame = next_frame_id();
        assert!(second_frame > first_frame);
        logger
            .record_frame(first_frame, "render-start", "first")
            .unwrap();
        logger
            .record_frame(first_frame, "diff-output", "second")
            .unwrap();

        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
        let initial = fs::read_to_string(logger.path()).unwrap();
        assert!(initial.contains(&format!("frame={first_frame} phase=render-start")));
        assert!(initial.contains(&format!("frame={first_frame} phase=diff-output")));

        let mut latest_frame = second_frame;
        for _ in 0..16 {
            latest_frame = next_frame_id();
            logger
                .record_frame(latest_frame, "render-start", &"x".repeat(256))
                .unwrap();
        }
        let text = fs::read_to_string(logger.path()).unwrap();
        assert!(text.len() <= 1024);
        assert!(text.contains(&format!("frame={latest_frame} phase=render-start")));
        assert!(!text.contains(&format!("frame={first_frame} phase=render-start")));

        latest_frame = next_frame_id();
        logger
            .record_frame(latest_frame, "unicode", &"界".repeat(1024))
            .unwrap();
        let unicode_text = fs::read_to_string(logger.path()).unwrap();
        assert!(unicode_text.len() <= 1024);
        assert!(unicode_text.contains(&format!("frame={latest_frame} phase=unicode")));
    }

    #[test]
    fn width_crash_artifacts_are_unique_one_shot_files() {
        let lines = vec!["too wide".to_owned()];
        let first = write_width_crash_log(1, &lines, 0).unwrap();
        let second = write_width_crash_log(1, &lines, 0).unwrap();

        assert_ne!(first, second);
        assert!(first.exists());
        assert!(second.exists());
        fs::remove_file(first).unwrap();
        fs::remove_file(second).unwrap();
    }
}
