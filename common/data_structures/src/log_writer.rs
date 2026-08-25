//! A background-thread file writer so a hot request path can queue a log
//! line and return immediately, instead of blocking on disk I/O and
//! contending with other callers over a shared lock.

use std::{
    fs::File,
    io::{self, Write},
    sync::mpsc::{self, Sender},
    thread::{self, JoinHandle},
};

/// A cheaply-cloneable handle to a background thread that owns a [`File`]
/// and writes to it on callers' behalf.
///
/// Cloning shares the same background thread; [`LogWriter::write_all`]
/// queues its bytes and returns without waiting for the write to land on
/// disk. Because the write happens later, on another thread, a failure
/// writing to the file can't be reported back to the caller that queued
/// it — only a disconnected channel (the background thread having already
/// stopped) is reported as an error here.
#[derive(Clone)]
pub struct LogWriter {
    /// Queues a buffer for the background thread to write.
    sender: Sender<Vec<u8>>,
}

impl LogWriter {
    /// Spawns the background thread that owns `file`, and returns a handle
    /// to it plus the thread's [`JoinHandle`]. Drop every clone of the
    /// returned handle to let the background thread drain any
    /// still-queued writes and exit; join the handle afterward to wait for
    /// that to finish.
    #[must_use]
    #[inline]
    pub fn spawn(mut file: File) -> (Self, JoinHandle<()>) {
        let (sender, receiver) = mpsc::channel::<Vec<u8>>();

        let handle = thread::spawn(move || {
            for buf in receiver {
                let _ignored = file.write_all(&buf);
            }
        });

        (Self { sender }, handle)
    }

    /// Queues `buf` to be written to the underlying file, returning
    /// immediately without waiting for the write to complete.
    ///
    /// # Errors
    ///
    /// Returns an error only if the background thread has already stopped
    /// (its [`JoinHandle`] was joined, or it panicked).
    #[inline]
    pub fn write_all(&self, buf: &[u8]) -> io::Result<()> {
        self.sender.send(buf.to_vec()).map_err(|_send_error| {
            io::Error::new(io::ErrorKind::BrokenPipe, "log writer thread has stopped")
        })
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, thread};

    use super::LogWriter;

    #[test]
    fn write_all_delivers_bytes_to_the_file() {
        let mut file = tempfile::tempfile().unwrap();
        let (writer, handle) = LogWriter::spawn(file.try_clone().unwrap());

        writer.write_all(b"first\n").unwrap();
        writer.write_all(b"second\n").unwrap();

        drop(writer);
        handle.join().unwrap();

        let mut contents = String::new();
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)).unwrap();
        file.read_to_string(&mut contents).unwrap();

        assert_eq!(contents, "first\nsecond\n");
    }

    #[test]
    fn concurrent_writers_do_not_interleave_or_corrupt_individual_writes() {
        let mut file = tempfile::tempfile().unwrap();
        let (writer, handle) = LogWriter::spawn(file.try_clone().unwrap());

        let threads: Vec<_> = (0_u8..8)
            .map(|thread_id| {
                let writer = writer.clone();
                thread::spawn(move || {
                    for _ in 0..50 {
                        writer
                            .write_all(format!("line-from-{thread_id}\n").as_bytes())
                            .unwrap();
                    }
                })
            })
            .collect();

        for thread in threads {
            thread.join().unwrap();
        }

        drop(writer);
        handle.join().unwrap();

        let mut contents = String::new();
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0)).unwrap();
        file.read_to_string(&mut contents).unwrap();

        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 400);
        for thread_id in 0_u8..8 {
            let expected = format!("line-from-{thread_id}");
            assert_eq!(
                lines.iter().filter(|&&line| line == expected).count(),
                50,
                "expected 50 intact lines from thread {thread_id}"
            );
        }
    }
}
