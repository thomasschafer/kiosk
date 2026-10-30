//! Finding the sequences that change program status in a terminal's raw output.

/// The protocol's cap on a whole sequence, `OSC` through `ST`.
const MAX_SEQUENCE: usize = 4096;
/// `OSC` is two bytes and `ST` at least one.
const MAX_OSC_BODY: usize = MAX_SEQUENCE - 3;

const ESC: u8 = 0x1b;
const BEL: u8 = 0x07;
const CAN: u8 = 0x18;
const SUB: u8 = 0x1a;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sequence {
    /// The body of an `OSC 7501` report, after `7501;`.
    ProgramStatus(String),
    /// `OSC 133 ; A`: a shell prompt began.
    PromptStart,
    /// `ESC c`: a full reset.
    Reset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Osc { overflowed: bool },
    OscEscape { overflowed: bool },
}

pub struct Scanner {
    state: State,
    body: Vec<u8>,
}

impl Default for Scanner {
    fn default() -> Self {
        Self {
            state: State::Ground,
            body: Vec::new(),
        }
    }
}

impl Scanner {
    /// Feeds the next bytes of output, calling `found` for each complete sequence.
    pub fn feed(&mut self, bytes: &[u8], found: &mut impl FnMut(Sequence)) {
        for &byte in bytes {
            self.byte(byte, found);
        }
    }

    fn byte(&mut self, byte: u8, found: &mut impl FnMut(Sequence)) {
        self.state = match self.state {
            State::Ground => {
                if byte == ESC {
                    State::Escape
                } else {
                    State::Ground
                }
            }
            State::Escape => self.after_escape(byte, found),
            State::Osc { overflowed } => match byte {
                BEL => {
                    self.finish(overflowed, found);
                    State::Ground
                }
                ESC => State::OscEscape { overflowed },
                CAN | SUB => State::Ground,
                _ => {
                    let overflowed = overflowed || self.body.len() == MAX_OSC_BODY;
                    if !overflowed {
                        self.body.push(byte);
                    }
                    State::Osc { overflowed }
                }
            },
            State::OscEscape { overflowed } => {
                if byte == b'\\' {
                    self.finish(overflowed, found);
                    State::Ground
                } else {
                    // An escape inside an OSC abandons it and starts a new sequence.
                    self.after_escape(byte, found)
                }
            }
        };
    }

    fn after_escape(&mut self, byte: u8, found: &mut impl FnMut(Sequence)) -> State {
        match byte {
            b']' => {
                self.body.clear();
                State::Osc { overflowed: false }
            }
            b'c' => {
                found(Sequence::Reset);
                State::Ground
            }
            ESC => State::Escape,
            _ => State::Ground,
        }
    }

    fn finish(&mut self, overflowed: bool, found: &mut impl FnMut(Sequence)) {
        let body = std::mem::take(&mut self.body);
        if overflowed {
            return;
        }
        if let Some(report) = body.strip_prefix(b"7501;") {
            // Every legal report is ASCII; anything else is malformed.
            if let Ok(report) = std::str::from_utf8(report) {
                found(Sequence::ProgramStatus(report.to_owned()));
            }
        } else if body == b"133;A" || body.starts_with(b"133;A;") {
            found(Sequence::PromptStart);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(chunks: &[&[u8]]) -> Vec<Sequence> {
        let mut scanner = Scanner::default();
        let mut found = Vec::new();
        for chunk in chunks {
            scanner.feed(chunk, &mut |sequence| found.push(sequence));
        }
        found
    }

    #[test]
    fn finds_reports_with_either_terminator_across_chunks() {
        assert_eq!(
            scan(&[
                b"text\x1b]75",
                b"01;state=idle\x1b",
                b"\\more\x1b]7501;state=done\x07"
            ]),
            [
                Sequence::ProgramStatus("state=idle".into()),
                Sequence::ProgramStatus("state=done".into()),
            ]
        );
    }

    #[test]
    fn finds_prompts_and_resets_but_not_other_sequences() {
        assert_eq!(
            scan(&[
                b"\x1b]0;title\x07\x1b]133;A\x07\x1b]133;B\x07\x1b[31m\x1bc\x1b]133;A;cl=m\x1b\\"
            ]),
            [
                Sequence::PromptStart,
                Sequence::Reset,
                Sequence::PromptStart
            ]
        );
    }

    #[test]
    fn oversized_and_interrupted_sequences_are_dropped() {
        let mut long = b"\x1b]7501;state=idle:x=".to_vec();
        long.extend(std::iter::repeat_n(b'a', MAX_SEQUENCE));
        long.extend(b"\x07\x1b]7501;state=done\x07");
        assert_eq!(
            scan(&[&long]),
            [Sequence::ProgramStatus("state=done".into())]
        );
        assert_eq!(
            scan(&[b"\x1b]7501;state=idle\x18\x1b]7501;state=working\x1b]7501;state=done\x07"]),
            [Sequence::ProgramStatus("state=done".into())]
        );
    }
}
