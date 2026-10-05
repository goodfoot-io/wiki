//! The CLI's only path to the standard streams.
//!
//! Stdout carries the command's answer, so every write to it is fallible and
//! propagates with `?`. A reader that closes early (`wiki … | head -0`)
//! surfaces as the typed [`StdoutClosed`] error, which `main` recognises and
//! answers with a silent non-zero exit: the reader is gone, and the exit
//! code keeps the run fail-closed. Every other I/O error stays an ordinary
//! error. The type, not the [`io::ErrorKind`], carries the meaning: a
//! `BrokenPipe` from anywhere else (a child process's stdin, say) is a real
//! failure and must still be reported.
//!
//! Stderr carries diagnostics and goes through [`stderr`] / [`stderr_line`].
//! `print!`/`println!`/`eprint!`/`eprintln!` are denied crate-wide
//! (`clippy::print_stdout`, `clippy::print_stderr`) because they panic on a
//! failed write.

use std::fmt;
use std::io::{self, Write as _};

use miette::{IntoDiagnostic, Report, Result};

/// Stdout was closed by its reader before the command finished writing.
#[derive(Debug, thiserror::Error, miette::Diagnostic)]
#[error("stdout closed by the reader")]
pub struct StdoutClosed(#[source] pub io::Error);

/// Map the result of a stdout write into the CLI's error type.
pub trait StdoutResultExt<T> {
    /// `BrokenPipe` becomes [`StdoutClosed`]; any other error becomes an
    /// ordinary diagnostic, exactly as `into_diagnostic` would make it. Call
    /// this only on writes whose destination is the process's stdout.
    fn on_stdout(self) -> Result<T>;
}

impl<T> StdoutResultExt<T> for io::Result<T> {
    fn on_stdout(self) -> Result<T> {
        match self {
            Err(err) if err.kind() == io::ErrorKind::BrokenPipe => {
                Err(Report::new(StdoutClosed(err)))
            }
            other => other.into_diagnostic(),
        }
    }
}

/// True when `report`, or any error in its chain, is [`StdoutClosed`].
pub fn is_stdout_closed(report: &Report) -> bool {
    report.chain().any(|err| err.is::<StdoutClosed>())
}

/// A locked handle on the process's stdout whose writes map through
/// [`StdoutResultExt::on_stdout`]. The inherent `write_fmt` makes the std
/// `write!`/`writeln!` macros return [`Result`] directly, so a call site
/// cannot forget the mapping. Std's line buffering is kept as is: each
/// complete line reaches the stream when written, preserving its order
/// relative to stderr diagnostics. Call [`Stdout::flush`] before returning so
/// a failure on the last (possibly partial) line is reported, not swallowed
/// by the exit-time flush.
///
/// Hold it only for an output phase, never across work that may spawn
/// threads: std's stdout lock is a reentrant mutex, so another thread
/// writing to stdout would block until it is released.
pub struct Stdout(io::StdoutLock<'static>);

impl Stdout {
    pub fn lock() -> Self {
        Self(io::stdout().lock())
    }

    /// Target of the `write!` / `writeln!` macros, which expand to
    /// `dst.write_fmt(format_args!(..))` and so resolve to this inherent
    /// method, returning its [`Result`]. Keep it inherent: implementing
    /// [`io::Write`] for `Stdout` instead would hand those macros an
    /// `io::Result` and silently drop the [`StdoutClosed`] mapping.
    pub fn write_fmt(&mut self, args: fmt::Arguments<'_>) -> Result<()> {
        self.0.write_fmt(args).on_stdout()
    }

    pub fn flush(&mut self) -> Result<()> {
        self.0.flush().on_stdout()
    }

    /// The raw lock, for helpers generic over [`io::Write`]; map their
    /// result with [`StdoutResultExt::on_stdout`].
    pub fn raw(&mut self) -> &mut io::StdoutLock<'static> {
        &mut self.0
    }
}

/// Write `args` to stderr, ignoring any write error.
///
/// Stderr is the last-resort channel: when a diagnostic cannot be written
/// there, there is nowhere left to report that failure, and failing the run
/// over it would turn a lost warning into a lost answer. The exit code still
/// carries the verdict. This is deliberately the one place the CLI drops a
/// write error.
pub fn stderr(args: fmt::Arguments<'_>) {
    let _ = io::stderr().lock().write_fmt(args);
}

/// [`stderr`] followed by a newline, under one lock (the `eprintln!`
/// replacement).
pub fn stderr_line(args: fmt::Arguments<'_>) {
    stderr(format_args!("{args}\n"));
}

#[cfg(test)]
mod tests {
    use super::*;
    use miette::WrapErr;

    /// A genuine `EPIPE`: write to a pipe whose reader is already gone. Rust
    /// ignores `SIGPIPE`, so the write returns `BrokenPipe`.
    fn broken_pipe_write() -> io::Result<()> {
        let (reader, mut writer) = io::pipe().expect("create pipe");
        drop(reader);
        writer.write_all(b"answer\n")
    }

    #[test]
    fn stdout_broken_pipe_maps_to_stdout_closed() {
        let result = broken_pipe_write();
        assert_eq!(
            result.as_ref().map_err(io::Error::kind).err(),
            Some(io::ErrorKind::BrokenPipe)
        );
        let Err(report) = result.on_stdout() else {
            panic!("a broken pipe is an error");
        };
        assert!(is_stdout_closed(&report), "{report:?}");
        assert!(report.downcast_ref::<StdoutClosed>().is_some());
    }

    #[test]
    fn stdout_closed_is_found_under_added_context() {
        let Err(report) = broken_pipe_write()
            .on_stdout()
            .wrap_err("while writing the results")
        else {
            panic!("a broken pipe is an error");
        };
        assert!(is_stdout_closed(&report), "{report:?}");
    }

    #[test]
    fn non_stdout_broken_pipe_is_not_stdout_closed() {
        // A broken pipe to anything but stdout (a child's stdin, say) takes
        // the ordinary error path and must stay reportable.
        let Err(report) = broken_pipe_write().into_diagnostic() else {
            panic!("a broken pipe is an error");
        };
        assert!(!is_stdout_closed(&report), "{report:?}");
    }

    #[test]
    fn other_stdout_errors_stay_ordinary() {
        let result: io::Result<()> = Err(io::Error::other("disk full"));
        let Err(report) = result.on_stdout() else {
            panic!("an error stays an error");
        };
        assert!(!is_stdout_closed(&report), "{report:?}");
        assert_eq!(report.to_string(), "disk full");
    }
}
