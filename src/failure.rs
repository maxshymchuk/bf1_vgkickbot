use crossterm::style::{Attribute, Color, ResetColor, SetAttribute, SetForegroundColor};
use std::error::Error;
use std::fmt;
use std::io::{self, BufRead, IsTerminal, Write};
use std::panic::{self, AssertUnwindSafe};
use std::process::ExitCode;
use tokio::sync::mpsc;

pub(crate) type PanicReceiver = mpsc::UnboundedReceiver<String>;

#[derive(Debug)]
struct ErrorMessage {
    message: String,
    error: io::Error,
}

impl fmt::Display for ErrorMessage {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(formatter)
    }
}

impl Error for ErrorMessage {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.error)
    }
}

pub(crate) fn with_message(message: impl Into<String>, error: impl Into<io::Error>) -> io::Error {
    let error = error.into();
    io::Error::new(
        error.kind(),
        ErrorMessage {
            message: message.into(),
            error,
        },
    )
}

fn panic_error(details: String) -> io::Error {
    with_message(
        "The bot stopped because of an unexpected internal error.\nInclude the details below when reporting this problem.",
        io::Error::other(details),
    )
}

fn catch_failure(work: impl FnOnce() -> io::Result<()>) -> io::Result<()> {
    panic::catch_unwind(AssertUnwindSafe(work))
        .unwrap_or_else(|_| Err(panic_error("Unexpected Rust panic.".to_string())))
}

pub(crate) fn run_guarded(
    work: impl FnOnce(&mut PanicReceiver) -> io::Result<()>,
) -> io::Result<()> {
    let (sender, mut receiver) = mpsc::unbounded_channel();
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        // Panic payloads can contain Debug dumps of credentials or HTTP headers.
        // Report the source location without exposing those instance values.
        let message = match info.location() {
            Some(location) => format!("Unexpected Rust panic at {location}."),
            None => "Unexpected Rust panic.".to_string(),
        };
        let _ = sender.send(message);
    }));
    let result = catch_failure(|| work(&mut receiver));
    let result = match receiver.try_recv() {
        Ok(message) => Err(panic_error(message)),
        Err(_) => result,
    };
    panic::set_hook(previous_hook);
    result
}

pub(crate) async fn supervise(
    work: impl std::future::Future<Output = io::Result<()>>,
    receiver: &mut PanicReceiver,
) -> io::Result<()> {
    tokio::select! {
        biased;
        message = receiver.recv() => Err(panic_error(
            message.unwrap_or_else(|| "Panic reporting channel closed unexpectedly.".to_string())
        )),
        result = work => result,
    }
}

fn report(error: &io::Error, output: &mut impl Write, styled: bool) -> io::Result<()> {
    let message = error
        .get_ref()
        .and_then(|source| source.downcast_ref::<ErrorMessage>())
        .map(|context| context.message.as_str())
        .unwrap_or("The bot could not continue. See the details below.");
    writeln!(output)?;
    if styled {
        crossterm::queue!(
            output,
            SetForegroundColor(Color::Red),
            SetAttribute(Attribute::Bold)
        )?;
    }
    writeln!(output, "[ APPLICATION ERROR ]")?;
    if styled {
        crossterm::queue!(output, SetAttribute(Attribute::Reset), ResetColor)?;
    }
    writeln!(output, "{message}\n")?;
    writeln!(output, "{error}\n")?;
    write!(output, "Press Enter to close...")?;
    output.flush()
}

fn report_and_wait(
    error: &io::Error,
    input: &mut impl BufRead,
    output: &mut impl Write,
    styled: bool,
) -> io::Result<()> {
    let display_result = report(error, output, styled);
    let mut line = String::new();
    // Still wait if writing to the console fails.
    let wait_result = input.read_line(&mut line).map(|_| ());
    display_result.and(wait_result)
}

pub(crate) fn finish(result: io::Result<()>) -> ExitCode {
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            let stderr = io::stderr();
            let _ = report_and_wait(
                &error,
                &mut io::stdin().lock(),
                &mut stderr.lock(),
                stderr.is_terminal(),
            );
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, ErrorKind};

    #[test]
    fn every_error_kind_prints_the_error_and_consumes_enter() {
        for kind in [
            ErrorKind::InvalidData,
            ErrorKind::InvalidInput,
            ErrorKind::NotFound,
            ErrorKind::ConnectionRefused,
            ErrorKind::Other,
        ] {
            let mut input = Cursor::new(b"\nremaining input".to_vec());
            let mut output = Vec::new();
            report_and_wait(
                &io::Error::new(kind, "failure details"),
                &mut input,
                &mut output,
                false,
            )
            .unwrap();
            assert_eq!(input.position(), 1);
            let text = String::from_utf8(output).unwrap();
            assert!(text.contains("failure details"));
            assert!(text.contains("Press Enter to close..."));
        }
    }

    #[test]
    fn panic_becomes_an_error_that_also_waits_for_enter() {
        let error = catch_failure(|| panic!("test panic payload")).unwrap_err();
        let mut input = Cursor::new(b"\n".to_vec());
        let mut output = Vec::new();
        report_and_wait(&error, &mut input, &mut output, false).unwrap();
        assert_eq!(input.position(), 1);
        assert!(String::from_utf8(output)
            .unwrap()
            .contains("Unexpected Rust panic"));
    }

    #[test]
    fn background_panic_notification_stops_monitoring_future() {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let (sender, mut receiver) = mpsc::unbounded_channel();
            tokio::spawn(async move {
                sender
                    .send("Unexpected Rust panic in worker".to_string())
                    .unwrap();
            });
            let result = supervise(std::future::pending(), &mut receiver).await;
            assert!(result.unwrap_err().to_string().contains("worker"));
        });
    }

    #[test]
    fn closed_input_does_not_hang_error_reporting() {
        let mut input = Cursor::new(Vec::<u8>::new());
        report_and_wait(
            &io::Error::other("failure"),
            &mut input,
            &mut Vec::new(),
            false,
        )
        .unwrap();
    }

    #[test]
    fn explanatory_message_preserves_original_error_and_enter_prompt() {
        let error = with_message(
            "Could not authenticate with EA.\nCheck sid/remid.",
            io::Error::new(ErrorKind::ConnectionRefused, "original network failure"),
        );
        assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
        assert_eq!(error.to_string(), "original network failure");
        let mut input = Cursor::new(b"\n".to_vec());
        let mut output = Vec::new();
        report_and_wait(&error, &mut input, &mut output, false).unwrap();
        assert_eq!(input.position(), 1);
        assert_eq!(String::from_utf8(output).unwrap(),
            "\n[ APPLICATION ERROR ]\nCould not authenticate with EA.\nCheck sid/remid.\n\noriginal network failure\n\nPress Enter to close...");
    }
}
