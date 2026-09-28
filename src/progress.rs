//! One status line per selected category when stderr is a terminal.

use std::io::{self, Write};
use std::sync::Mutex;

pub fn bar(done: u64, total: u64) -> String {
    const WIDTH: usize = 20;
    if total == 0 || done == 0 {
        return " ".repeat(WIDTH);
    }
    if done >= total {
        return "=".repeat(WIDTH);
    }
    let equals = ((done * WIDTH as u64) / total) as usize;
    let mut text = "=".repeat(equals);
    text.push('>');
    while text.chars().count() < WIDTH {
        text.push(' ');
    }
    text
}

pub fn format_line(category: &str, done: u64, total: u64, label: &str) -> String {
    format!(
        "{:<8} {}/{} [{}] {}",
        category,
        done,
        total,
        bar(done, total),
        label
    )
}

#[derive(Clone, Copy)]
enum SinkKind {
    Stderr,
    Buffer,
}

struct SinkState {
    kind: SinkKind,
    is_terminal: bool,
    drawn: usize,
    buf: Vec<u8>,
}

#[derive(Clone)]
pub struct SharedSink {
    state: std::sync::Arc<Mutex<SinkState>>,
}

impl SharedSink {
    pub fn stderr() -> Self {
        Self::new(SinkKind::Stderr, io::IsTerminal::is_terminal(&io::stderr()))
    }

    pub fn buffer(is_terminal: bool) -> Self {
        Self::new(SinkKind::Buffer, is_terminal)
    }

    fn new(kind: SinkKind, is_terminal: bool) -> Self {
        Self {
            state: std::sync::Arc::new(Mutex::new(SinkState {
                kind,
                is_terminal,
                drawn: 0,
                buf: Vec::new(),
            })),
        }
    }

    pub fn write_progress(&self, lines: &[String]) {
        let mut state = self.state.lock().expect("progress lock");
        if !state.is_terminal {
            return;
        }
        let mut block = String::new();
        if state.drawn > 0 {
            block.push_str(&format!("\x1b[{}A\r", state.drawn));
        }
        for (index, line) in lines.iter().enumerate() {
            if index > 0 {
                block.push('\n');
            }
            block.push_str("\x1b[K");
            block.push_str(line);
        }
        block.push('\n');
        state.drawn = lines.len();
        state.write(&block);
    }

    pub fn write_stderr(&self, text: &str) {
        let mut state = self.state.lock().expect("progress lock");
        state.drawn = 0;
        state.write(text);
    }

    pub fn captured(&self) -> String {
        let state = self.state.lock().expect("progress lock");
        String::from_utf8_lossy(&state.buf).into_owned()
    }
}

impl SinkState {
    fn write(&mut self, text: &str) {
        match self.kind {
            SinkKind::Stderr => {
                let mut err = io::stderr().lock();
                let _ = err.write_all(text.as_bytes());
                let _ = err.flush();
            }
            SinkKind::Buffer => self.buf.extend_from_slice(text.as_bytes()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formatter_matches_the_three_line_example() {
        let lines = [
            format_line("gmail", 2, 3, "2026-09-27"),
            format_line("messages", 1, 3, "2026-09-26"),
            format_line("calendar", 0, 3, "waiting"),
        ];
        assert_eq!(lines[0], "gmail    2/3 [=============>      ] 2026-09-27");
        assert_eq!(lines[1], "messages 1/3 [======>             ] 2026-09-26");
        assert_eq!(lines[2], "calendar 0/3 [                    ] waiting");
        assert_eq!(bar(3, 3), "=".repeat(20));
        assert_eq!(bar(0, 3), " ".repeat(20));
    }

    #[test]
    fn non_terminal_sink_records_no_progress_text() {
        let sink = SharedSink::buffer(false);
        sink.write_progress(&[format_line("gmail", 1, 3, "2026-09-26")]);
        assert!(sink.captured().is_empty());
        sink.write_stderr("2026-09-26 failed gmail: nope\n");
        assert_eq!(sink.captured(), "2026-09-26 failed gmail: nope\n");
    }
}
