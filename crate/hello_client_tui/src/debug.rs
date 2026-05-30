use std::{collections::VecDeque, fs, io, io::Write, path::Path, time::Instant};

/// Maximum number of entries kept in memory.
const MAX_ENTRIES: usize = 500;

/// Timestamped debug log.
///
/// Entries are always written to the in-memory ring buffer. When constructed
/// with a `path`, they are also flushed to that file after every write so the
/// file is readable even if the process is killed.
pub struct DebugLog {
    entries: VecDeque<String>,
    file: Option<io::BufWriter<fs::File>>,
    start: Instant,
}

impl DebugLog {
    /// Create a new log. Pass `Some(path)` to mirror output to a file.
    pub fn new(path: Option<&Path>) -> io::Result<Self> {
        let file = path.map(|p| fs::File::create(p).map(io::BufWriter::new)).transpose()?;
        Ok(Self {
            entries: VecDeque::with_capacity(MAX_ENTRIES),
            file,
            start: Instant::now(),
        })
    }

    /// Append a message, prefixed with `+MM:SS.mmm` elapsed time.
    pub fn log(&mut self, msg: impl AsRef<str>) {
        let elapsed = self.start.elapsed();
        let secs = elapsed.as_secs();
        let entry = format!(
            "+{:02}:{:02}.{:03}  {}",
            secs / 60,
            secs % 60,
            elapsed.subsec_millis(),
            msg.as_ref()
        );

        if self.entries.len() >= MAX_ENTRIES {
            self.entries.pop_front();
        }
        self.entries.push_back(entry.clone());

        if let Some(f) = &mut self.file {
            let _ = writeln!(f, "{}", entry);
            let _ = f.flush();
        }
    }

    pub fn entries(&self) -> &VecDeque<String> {
        &self.entries
    }

    /// `true` if output is being mirrored to a file.
    pub fn has_file(&self) -> bool {
        self.file.is_some()
    }
}
