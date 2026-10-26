//! Sets up logging, to daily log files or to standard output.

use std::fmt;

use serde::Deserialize;
use tracing::level_filters::LevelFilter;
use tracing::{Event, Level, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::EnvFilter;
use tracing_subscriber::fmt::format::Writer;
use tracing_subscriber::fmt::{FmtContext, FormatEvent, FormatFields};
use tracing_subscriber::registry::LookupSpan;

/// Where log messages go.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogOutput {
    /// To systemd's journal when it is connected to standard output, and to files otherwise.
    #[default]
    Auto,
    /// To daily log files in the log directory.
    File,
    /// To standard output.
    Stdout,
}

/// The settings for [init].
pub struct LogConfiguration<'a> {
    pub output: LogOutput,
    /// The prefix of the log file names, which end with the date.
    pub file_name: &'a str,
    /// How many daily log files are kept, or 0 to keep them all.
    pub retention_days: usize,
}

/// Starts logging. The returned guard must be kept until the server is done, as dropping it
/// writes out the remaining messages and stops logging. The log level is info, unless the
/// RUST_LOG environment variable says otherwise.
pub fn init(config: LogConfiguration<'_>) -> anyhow::Result<WorkerGuard> {
    let filter = EnvFilter::builder()
        .with_default_directive(LevelFilter::INFO.into())
        .from_env_lossy();
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_line_number(false)
        .with_file(false)
        .with_target(false);

    let journal = stdout_is_journal();
    let to_stdout = match config.output {
        LogOutput::Auto => journal,
        LogOutput::File => false,
        LogOutput::Stdout => true,
    };

    if !to_stdout {
        let mut appender = RollingFileAppender::builder()
            .rotation(Rotation::DAILY)
            .filename_prefix(config.file_name);
        if config.retention_days > 0 {
            appender = appender.max_log_files(config.retention_days);
        }
        let (writer, guard) = tracing_appender::non_blocking(appender.build("log")?);
        builder.with_ansi(false).with_writer(writer).init();
        return Ok(guard);
    }

    let (writer, guard) = tracing_appender::non_blocking(std::io::stdout());
    if journal {
        // The journal adds its own timestamps, and takes the priority from the line's prefix.
        let format = JournalFormat(
            tracing_subscriber::fmt::format()
                .without_time()
                .with_level(false)
                .with_target(false),
        );
        builder
            .with_ansi(false)
            .with_writer(writer)
            .event_format(format)
            .init();
    } else {
        let ansi = std::io::IsTerminal::is_terminal(&std::io::stdout());
        builder.with_ansi(ansi).with_writer(writer).init();
    }
    Ok(guard)
}

/// Formats events like the wrapped format, with the `<N>` priority prefix that the journal reads
/// from lines written to standard output, so that `journalctl -p warning` and colors work.
struct JournalFormat<F>(F);

impl<S, N, F> FormatEvent<S, N> for JournalFormat<F>
where
    S: Subscriber + for<'a> LookupSpan<'a>,
    N: for<'a> FormatFields<'a> + 'static,
    F: FormatEvent<S, N>,
{
    fn format_event(
        &self,
        ctx: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let priority = match *event.metadata().level() {
            Level::ERROR => 3,
            Level::WARN => 4,
            Level::INFO => 6,
            Level::DEBUG | Level::TRACE => 7,
        };
        write!(writer, "<{priority}>")?;
        self.0.format_event(ctx, writer, event)
    }
}

/// Whether standard output is connected to systemd's journal, which systemd says with the device
/// and inode numbers of the stream in JOURNAL_STREAM.
#[cfg(unix)]
fn stdout_is_journal() -> bool {
    use std::os::fd::AsFd;
    use std::os::unix::fs::MetadataExt;

    let Some(stream) = std::env::var_os("JOURNAL_STREAM") else {
        return false;
    };
    let Some((dev, ino)) = stream.to_str().and_then(|s| s.split_once(':')) else {
        return false;
    };
    let (Ok(dev), Ok(ino)) = (dev.parse::<u64>(), ino.parse::<u64>()) else {
        return false;
    };
    let Ok(stdout) = std::io::stdout().as_fd().try_clone_to_owned() else {
        return false;
    };
    std::fs::File::from(stdout)
        .metadata()
        .is_ok_and(|metadata| metadata.dev() == dev && metadata.ino() == ino)
}

#[cfg(not(unix))]
fn stdout_is_journal() -> bool {
    false
}
