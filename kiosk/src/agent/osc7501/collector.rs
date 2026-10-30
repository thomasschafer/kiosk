//! The process tmux pipes one pane's output to (`kiosk collect`). tmux shows programs'
//! OSC 7501 reports to nobody, so the collector reads them from the raw output and keeps
//! the pane's most urgent record in a pane option, where the picker reads it.

use std::io::Read;

use super::{
    records::RecordSet,
    report::{self, Record},
    scanner::{Scanner, Sequence},
};
use crate::{
    args,
    tmux::{COLLECTOR_OPTION, PaneId, STATUS_OPTION, Tmux, TmuxError},
};

/// Where the collector reports the pane's status.
pub trait Publish {
    fn publish(&mut self, record: Option<&Record>) -> Result<(), TmuxError>;
}

/// Turns output into published status, publishing only changes.
pub struct Collector<P> {
    scanner: Scanner,
    records: RecordSet,
    published: Option<Record>,
    publisher: P,
}

impl<P: Publish> Collector<P> {
    pub fn new(publisher: P) -> Self {
        Self {
            scanner: Scanner::default(),
            records: RecordSet::default(),
            published: None,
            publisher,
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) -> Result<(), TmuxError> {
        let records = &mut self.records;
        self.scanner.feed(bytes, &mut |sequence| match sequence {
            Sequence::ProgramStatus(body) => {
                if let Some(report) = report::parse(&body) {
                    records.apply(report);
                }
            }
            Sequence::PromptStart => records.prompt_started(),
            Sequence::Reset => records.reset(),
        });
        let current = self.records.most_urgent();
        if current != self.published {
            self.publisher.publish(current.as_ref())?;
            self.published = current;
        }
        Ok(())
    }
}

/// Publishes to a pane's options.
pub struct PaneOptions<'a> {
    tmux: &'a dyn Tmux,
    pane: PaneId,
}

impl Publish for PaneOptions<'_> {
    fn publish(&mut self, record: Option<&Record>) -> Result<(), TmuxError> {
        let pane = self.pane.as_str();
        let command = match record {
            Some(record) => args![
                "set-option",
                "-p",
                "-t",
                pane,
                STATUS_OPTION,
                record.to_string()
            ],
            None => args!["set-option", "-p", "-u", "-t", pane, STATUS_OPTION],
        };
        self.tmux.run(command).map(drop)
    }
}

/// Collects `input`, the output of `pane`, until it ends, then removes what it published.
/// The collector option holds this process's id, so a collector that ends after another
/// has taken the pane over leaves the newer one's options alone.
pub fn run(tmux: &dyn Tmux, pane: &PaneId, mut input: impl Read) -> Result<(), TmuxError> {
    let id = std::process::id().to_string();
    tmux.run(args![
        "set-option",
        "-p",
        "-t",
        pane.as_str(),
        COLLECTOR_OPTION,
        id.clone()
    ])?;
    let mut collector = Collector::new(PaneOptions {
        tmux,
        pane: pane.clone(),
    });
    let mut buffer = [0; 8192];
    let result = loop {
        match input.read(&mut buffer) {
            Ok(0) => break Ok(()),
            Ok(read) => match collector.feed(&buffer[..read]) {
                Ok(()) | Err(TmuxError::Failed { .. }) => {}
                Err(error) => break Err(error),
            },
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                break Err(TmuxError::Invocation(format!(
                    "could not read the pane's output: {error}"
                )));
            }
        }
    };
    let pane = pane.as_str();
    let cleanup = args![
        "if-shell",
        "-F",
        "-t",
        pane,
        format!("#{{==:#{{{COLLECTOR_OPTION}}},{id}}}"),
        format!(
            "set-option -p -u -t {pane} {STATUS_OPTION} ; set-option -p -u -t {pane} {COLLECTOR_OPTION}"
        ),
    ];
    match tmux.run(cleanup) {
        // The pane, or the whole server, is gone along with its options.
        Ok(_) | Err(TmuxError::NoServer | TmuxError::Failed { .. }) => result,
        Err(error) => result.and(Err(error)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tmux::mock::MockTmux;

    #[derive(Default)]
    struct Recorded(Vec<Option<String>>);

    impl Publish for &mut Recorded {
        fn publish(&mut self, record: Option<&Record>) -> Result<(), TmuxError> {
            self.0.push(record.map(ToString::to_string));
            Ok(())
        }
    }

    #[test]
    fn only_changes_are_published() {
        let mut recorded = Recorded::default();
        let mut collector = Collector::new(&mut recorded);
        collector
            .feed(b"\x1b]7501;state=working:app=claude-code\x07output")
            .unwrap();
        collector
            .feed(b"\x1b]7501;state=working:app=claude-code:progress=40\x07")
            .unwrap();
        collector.feed(b"\x1b]133;A\x07$ ").unwrap();
        assert_eq!(
            recorded.0,
            [Some("state=working:app=claude-code".into()), None]
        );
    }

    #[test]
    fn a_run_marks_the_pane_publishes_and_cleans_up_its_own_options() {
        let tmux = MockTmux::default();
        for _ in 0..3 {
            tmux.respond(Ok(String::new()));
        }
        let pane = PaneId::parse("%4").unwrap();
        run(
            &tmux,
            &pane,
            &b"\x1b]7501;state=blocked:kind=auth\x1b\\"[..],
        )
        .unwrap();
        let id = std::process::id().to_string();
        let calls = tmux.calls();
        assert_eq!(
            calls[0],
            args![
                "set-option",
                "-p",
                "-t",
                "%4",
                "@kiosk_collector",
                id.clone()
            ]
        );
        assert_eq!(
            calls[1],
            args![
                "set-option",
                "-p",
                "-t",
                "%4",
                "@kiosk_status",
                "state=blocked:kind=auth"
            ]
        );
        assert_eq!(
            calls[2][..5],
            args![
                "if-shell",
                "-F",
                "-t",
                "%4",
                format!("#{{==:#{{@kiosk_collector}},{id}}}")
            ]
        );
        assert_eq!(calls.len(), 3);
    }
}
