//! Debug logging helpers extracted from `frame_differ`.
//!
//! These functions write diagnostic logs to `/tmp/neo-tui-debug/` when the
//! `NEO_TUI_DEBUG=1` environment variable is set.

use std::env;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::time::SystemTime;

use crate::primitive::visible_width;

use super::frame_differ::{DiffRender, RenderDimensions, TuiRenderer};

pub(super) fn debug_log_enabled() -> bool {
    env::var("NEO_TUI_DEBUG").is_ok_and(|v| v == "1")
}

pub(super) fn write_output_log(label: &str, buffer: &str) -> std::io::Result<()> {
    let mut file = create_debug_log_file(&format!("output-{label}"))?;
    file.write_all(buffer.as_bytes())?;
    file.flush()
}

fn create_debug_log_file(stem: &str) -> std::io::Result<fs::File> {
    let path = debug_log_path(stem)?;
    fs::File::create(path)
}

fn debug_log_path(stem: &str) -> std::io::Result<PathBuf> {
    let dir = PathBuf::from("/tmp/neo-tui-debug");
    fs::create_dir_all(&dir)?;
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    Ok(dir.join(format!("{stem}-{timestamp}.log")))
}

fn write_rendered_lines(
    file: &mut fs::File,
    heading: &str,
    lines: &[String],
) -> std::io::Result<()> {
    writeln!(file, "{heading}")?;
    for (index, line) in lines.iter().enumerate() {
        let width = visible_width(line);
        writeln!(file, "[{index}] (w={width}) {line}")?;
    }
    Ok(())
}

pub(super) fn write_debug_log(
    label: &str,
    width: u16,
    height: usize,
    new_lines: &[String],
    previous_lines: &[String],
    extra: Option<&str>,
) -> std::io::Result<()> {
    let mut file = create_debug_log_file(label)?;
    write_debug_log_header(&mut file, label, width, height, new_lines, previous_lines)?;
    write_optional_debug_text(&mut file, extra)?;
    write_debug_log_lines(&mut file, new_lines, previous_lines)?;
    file.flush()
}

fn write_debug_log_lines(
    file: &mut fs::File,
    new_lines: &[String],
    previous_lines: &[String],
) -> std::io::Result<()> {
    write_rendered_lines(file, "=== new_lines ===", new_lines)?;
    write_rendered_lines(file, "=== previous_lines ===", previous_lines)
}

fn write_debug_log_header(
    file: &mut fs::File,
    label: &str,
    width: u16,
    height: usize,
    new_lines: &[String],
    previous_lines: &[String],
) -> std::io::Result<()> {
    writeln!(file, "[{label}] width={width} height={height}")?;
    writeln!(
        file,
        "new_lines.len()={} previous_lines.len()={}",
        new_lines.len(),
        previous_lines.len()
    )
}

fn write_optional_debug_text(file: &mut fs::File, extra: Option<&str>) -> std::io::Result<()> {
    if let Some(text) = extra {
        writeln!(file, "{text}")?;
    }
    Ok(())
}

fn write_width_crash_log(
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<PathBuf> {
    let path = debug_log_path("width-crash")?;
    let mut file = fs::File::create(&path)?;
    write_width_crash_body(&mut file, width, new_lines, offender_idx)?;
    Ok(path)
}

fn write_width_crash_body(
    file: &mut fs::File,
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<()> {
    write_width_crash_header(file, width, new_lines, offender_idx)?;
    write_rendered_lines(file, "=== All rendered lines ===", new_lines)?;
    file.flush()
}

fn write_width_crash_header(
    file: &mut fs::File,
    width: u16,
    new_lines: &[String],
    offender_idx: usize,
) -> std::io::Result<()> {
    writeln!(file, "Terminal width: {width}")?;
    writeln!(file, "Offending line index: {offender_idx}")?;
    writeln!(
        file,
        "Offending line visible width: {}",
        visible_width(&new_lines[offender_idx])
    )
}

pub(super) fn check_line_widths(width: u16, new_lines: &[String]) -> std::io::Result<()> {
    for (i, line) in new_lines.iter().enumerate() {
        if visible_width(line) > usize::from(width) {
            let path = write_width_crash_log(width, new_lines, i)?;
            return Err(std::io::Error::other(format!(
                "rendered line {i} exceeds terminal width ({} > {width}). crash log: {}",
                visible_width(line),
                path.display()
            )));
        }
    }
    Ok(())
}

impl TuiRenderer {
    pub(super) fn log_render_start(&self, dimensions: RenderDimensions, new_lines: &[String]) {
        if !debug_log_enabled() {
            return;
        }

        let _ = write_debug_log(
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

        let _ = write_output_log("diff-render", &diff_render.buffer);
        let _ = write_debug_log(
            "diff-render",
            dimensions.width,
            dimensions.height,
            new_lines,
            &self.previous_lines,
            Some(&format!(
                "first_changed={} last_changed={} append_start={} prev_viewport_top={} viewport_top={} hardware_cursor_row={} move_target_row={move_target_row} render_end={render_end} final_cursor_row={final_cursor_row}",
                diff_render.change_range.first,
                diff_render.change_range.last,
                diff_render.change_range.append_start,
                diff_render.viewport.previous_top,
                diff_render.viewport.top,
                diff_render.viewport.hardware_cursor_row,
                move_target_row = diff_render.move_target_row,
                render_end = diff_render.render_end,
                final_cursor_row = diff_render.final_cursor_row
            )),
        );
    }
}
