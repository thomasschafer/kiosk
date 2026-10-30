//! Parsing one Program Status Protocol (OSC 7501) report, the text between `OSC 7501;` and
//! the string terminator. See <https://mitchellh.com/writing/program-status-osc7501>.

use std::fmt;

use base64::{
    Engine,
    engine::{DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
};

const MAX_KEY_LEN: usize = 16;
const MAX_MSG_ENCODED: usize = 2732;
const MAX_MSG_DECODED: usize = 2048;
const MAX_TITLE_ENCODED: usize = 256;
const MAX_TITLE_DECODED: usize = 192;
const MAX_APP_LEN: usize = 32;
const MAX_ID_LEN: usize = 128;
const MAX_ID_SEGMENT_LEN: usize = 32;
const MAX_ID_DEPTH: usize = 8;

const BASE64: GeneralPurpose = GeneralPurpose::new(
    &base64::alphabet::STANDARD,
    GeneralPurposeConfig::new().with_decode_padding_mode(DecodePaddingMode::Indifferent),
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProgramState {
    Idle,
    Working,
    Done,
    Blocked,
    Error,
}

impl ProgramState {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "idle" => Self::Idle,
            "working" => Self::Working,
            "done" => Self::Done,
            "blocked" => Self::Blocked,
            "error" => Self::Error,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Working => "working",
            Self::Done => "done",
            Self::Blocked => "blocked",
            Self::Error => "error",
        }
    }

    /// Whether the record ends with the program: at its exit or the next shell prompt.
    pub fn ends_with_program(self) -> bool {
        matches!(self, Self::Working | Self::Blocked)
    }

    /// Higher wants the user's attention more.
    pub fn urgency(self) -> u8 {
        match self {
            Self::Blocked => 4,
            Self::Error => 3,
            Self::Done => 2,
            Self::Working => 1,
            Self::Idle => 0,
        }
    }
}

/// What a blocked program is waiting for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BlockedKind {
    Permission,
    Question,
    Auth,
}

impl BlockedKind {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "permission" => Self::Permission,
            "question" => Self::Question,
            "auth" => Self::Auth,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Permission => "permission",
            Self::Question => "question",
            Self::Auth => "auth",
        }
    }
}

/// The parts of a record kiosk shows. Progress, titles and messages are checked, as the
/// protocol requires, but not kept.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Record {
    pub state: ProgramState,
    /// Only for [`ProgramState::Blocked`].
    pub kind: Option<BlockedKind>,
    pub app: Option<String>,
}

impl fmt::Display for Record {
    /// The record as a report body without an id, which [`parse`] reads back.
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "state={}", self.state.as_str())?;
        if let Some(kind) = self.kind {
            write!(formatter, ":kind={}", kind.as_str())?;
        }
        if let Some(app) = &self.app {
            write!(formatter, ":app={app}")?;
        }
        Ok(())
    }
}

/// A record id: `/`-separated segments. The root record has none.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Default)]
pub struct RecordId(Vec<String>);

impl RecordId {
    pub fn root() -> Self {
        Self::default()
    }

    /// Whether `self` is `ancestor` or below it.
    pub fn is_within(&self, ancestor: &Self) -> bool {
        self.0.starts_with(&ancestor.0)
    }

    /// The ids above this one, nearest first, ending with the root.
    pub fn ancestors(&self) -> impl Iterator<Item = Self> + '_ {
        (0..self.0.len())
            .rev()
            .map(|depth| Self(self.0[..depth].to_vec()))
    }

    fn parse(value: &str) -> Result<Self, Invalid> {
        if value.len() > MAX_ID_LEN {
            return Err(Invalid::Limit);
        }
        let segments: Vec<String> = value.split('/').map(str::to_owned).collect();
        if segments.len() > MAX_ID_DEPTH {
            return Err(Invalid::Limit);
        }
        for segment in &segments {
            if segment.len() > MAX_ID_SEGMENT_LEN {
                return Err(Invalid::Limit);
            }
            if segment.is_empty()
                || !segment
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
            {
                return Err(Invalid::Ignored);
            }
        }
        Ok(Self(segments))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Report {
    Set {
        id: RecordId,
        record: Record,
    },
    /// Removes `id` and every record below it; the root id removes every record.
    Clear {
        id: RecordId,
    },
    /// Feature detection, which only a terminal answers.
    Query,
}

/// Why a report has no effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Invalid {
    /// The report breaks a limit or fails to decode, so it is discarded whole.
    Limit,
    /// The report is ignored: no recognized state, or a malformed id.
    Ignored,
}

/// Parses a report body. `None` means the report has no effect.
pub fn parse(body: &str) -> Option<Report> {
    if body.trim() == "?" {
        return Some(Report::Query);
    }
    parse_pairs(body).ok()
}

fn parse_pairs(body: &str) -> Result<Report, Invalid> {
    let mut state = None;
    let mut id = None;
    let mut kind = None;
    let mut app = None;
    for pair in body.split(':') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        if key.len() > MAX_KEY_LEN {
            return Err(Invalid::Limit);
        }
        if key.is_empty() || !key.bytes().all(|byte| byte.is_ascii_lowercase()) {
            continue;
        }
        if !value.bytes().all(is_value_byte) {
            // Skipping a malformed id would address the root record instead.
            if key == "id" {
                return Err(Invalid::Ignored);
            }
            continue;
        }
        match key {
            "state" => state = Some(value),
            "id" => id = Some(value),
            "kind" => kind = Some(value),
            "app" => app = Some(value),
            "msg" => check_text(value, MAX_MSG_ENCODED, MAX_MSG_DECODED)?,
            "title" => check_text(value, MAX_TITLE_ENCODED, MAX_TITLE_DECODED)?,
            _ => {}
        }
    }
    let app = match app {
        Some(app) if app.len() > MAX_APP_LEN => return Err(Invalid::Limit),
        Some(app) if is_app(app) => Some(app.to_owned()),
        _ => None,
    };
    let id = id.map_or_else(|| Ok(RecordId::root()), RecordId::parse)?;
    match state.ok_or(Invalid::Ignored)? {
        "clear" => Ok(Report::Clear { id }),
        state => {
            let state = ProgramState::parse(state).ok_or(Invalid::Ignored)?;
            let kind = kind
                .filter(|_| state == ProgramState::Blocked)
                .and_then(BlockedKind::parse);
            Ok(Report::Set {
                id,
                record: Record { state, kind, app },
            })
        }
    }
}

fn is_value_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || b"_.,+/=-".contains(&byte)
}

fn is_app(app: &str) -> bool {
    !app.is_empty()
        && app
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"_.+-".contains(&byte))
}

/// Free text must be base64 of UTF-8 without control characters, within its limits.
fn check_text(value: &str, max_encoded: usize, max_decoded: usize) -> Result<(), Invalid> {
    if value.len() > max_encoded {
        return Err(Invalid::Limit);
    }
    let decoded = BASE64.decode(value).map_err(|_| Invalid::Limit)?;
    if decoded.len() > max_decoded {
        return Err(Invalid::Limit);
    }
    let text = String::from_utf8(decoded).map_err(|_| Invalid::Limit)?;
    if text.chars().any(char::is_control) {
        return Err(Invalid::Limit);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(body: &str) -> (RecordId, Record) {
        match parse(body) {
            Some(Report::Set { id, record }) => (id, record),
            other => panic!("expected a record from {body:?}, got {other:?}"),
        }
    }

    #[test]
    fn parses_the_spec_example() {
        let (id, record) = set(
            "state=blocked:kind=permission:app=terraform:msg=QXBwbHkgMyB0byBhZGQsIDEgdG8gY2hhbmdlLCAwIHRvIGRlc3Ryb3k/",
        );
        assert_eq!(id, RecordId::root());
        assert_eq!(
            record,
            Record {
                state: ProgramState::Blocked,
                kind: Some(BlockedKind::Permission),
                app: Some("terraform".into()),
            }
        );
    }

    #[test]
    fn malformed_pairs_are_skipped_and_the_last_value_wins() {
        let (_, record) =
            set(" state = idle : nokey : =x : Bad=1 : app=a!b : state=working : app=cargo ");
        assert_eq!(record.state, ProgramState::Working);
        assert_eq!(record.app.as_deref(), Some("cargo"));
    }

    #[test]
    fn reports_without_a_known_state_or_with_a_bad_id_are_ignored() {
        assert_eq!(parse("app=x"), None);
        assert_eq!(parse("state=sleeping"), None);
        assert_eq!(parse("state=idle:id=a//b"), None);
        assert_eq!(parse("state=idle:id=a%b"), None);
        assert_eq!(parse("?"), Some(Report::Query));
    }

    #[test]
    fn kind_only_applies_to_blocked_and_unknown_values_are_absent() {
        assert_eq!(set("state=working:kind=auth").1.kind, None);
        assert_eq!(set("state=blocked:kind=later").1.kind, None);
        assert_eq!(
            set("state=blocked:kind=auth").1.kind,
            Some(BlockedKind::Auth)
        );
    }

    #[test]
    fn limits_and_bad_text_discard_the_whole_report() {
        assert_eq!(parse(&format!("state=idle:{}=1", "k".repeat(17))), None);
        assert_eq!(parse(&format!("state=idle:app={}", "a".repeat(33))), None);
        assert_eq!(parse(&format!("state=idle:id={}", "a".repeat(33))), None);
        assert_eq!(
            parse(&format!("state=idle:id={}", ["a"; 9].join("/"))),
            None
        );
        assert_eq!(parse("state=idle:msg=A"), None);
        // "a\nb" contains a newline.
        assert_eq!(parse("state=idle:msg=YQpi"), None);
        assert_eq!(
            parse(&format!("state=idle:title={}", "QUFB".repeat(65))),
            None
        );
        assert!(parse("state=idle:msg=SGk").is_some());
    }

    #[test]
    fn clear_addresses_an_id_or_everything() {
        assert_eq!(
            parse("state=clear"),
            Some(Report::Clear {
                id: RecordId::root()
            })
        );
        let Some(Report::Clear { id }) = parse("state=clear:id=build/test") else {
            panic!("expected a clear");
        };
        assert!(id.is_within(&RecordId::parse("build").unwrap()));
        assert_eq!(
            id.ancestors().collect::<Vec<_>>(),
            [RecordId::parse("build").unwrap(), RecordId::root()]
        );
    }

    #[test]
    fn records_display_as_bodies_that_parse_back() {
        let record = Record {
            state: ProgramState::Blocked,
            kind: Some(BlockedKind::Question),
            app: Some("claude-code".into()),
        };
        assert_eq!(
            record.to_string(),
            "state=blocked:kind=question:app=claude-code"
        );
        assert_eq!(set(&record.to_string()).1, record);
    }
}
