use std::fmt;
use std::io::{self, BufWriter, Write};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::de::{self, Deserializer};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventKind {
    SessionStarted,
    ThreadCreated,
    ThreadDeleted,
    ThreadResolved,
    ThreadUnresolved,
    ReplyAdded,
    PromptSuggestDone,
    SessionDone,
}

impl EventKind {
    pub const ALL: [Self; 8] = [
        Self::SessionStarted,
        Self::ThreadCreated,
        Self::ThreadDeleted,
        Self::ThreadResolved,
        Self::ThreadUnresolved,
        Self::ReplyAdded,
        Self::PromptSuggestDone,
        Self::SessionDone,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            Self::SessionStarted => "session.started",
            Self::ThreadCreated => "thread.created",
            Self::ThreadDeleted => "thread.deleted",
            Self::ThreadResolved => "thread.resolved",
            Self::ThreadUnresolved => "thread.unresolved",
            Self::ReplyAdded => "reply.added",
            Self::PromptSuggestDone => "prompt.suggest_done",
            Self::SessionDone => "session.done",
        }
    }
}

impl fmt::Display for EventKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl Serialize for EventKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for EventKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let raw = String::deserialize(deserializer)?;

        match raw.as_str() {
            "session.started" => Ok(Self::SessionStarted),
            "thread.created" => Ok(Self::ThreadCreated),
            "thread.deleted" => Ok(Self::ThreadDeleted),
            "thread.resolved" => Ok(Self::ThreadResolved),
            "thread.unresolved" => Ok(Self::ThreadUnresolved),
            "reply.added" => Ok(Self::ReplyAdded),
            "prompt.suggest_done" => Ok(Self::PromptSuggestDone),
            "session.done" => Ok(Self::SessionDone),
            _ => Err(de::Error::unknown_variant(
                &raw,
                &[
                    "session.started",
                    "thread.created",
                    "thread.deleted",
                    "thread.resolved",
                    "thread.unresolved",
                    "reply.added",
                    "prompt.suggest_done",
                    "session.done",
                ],
            )),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Event {
    pub kind: EventKind,
    pub at: DateTime<Utc>,
    pub payload: Value,
}

pub struct EventEmitter<W: Write = io::Stdout> {
    writer: Mutex<BufWriter<W>>,
}

impl EventEmitter<Box<dyn Write + Send>> {
    pub fn boxed<W>(writer: W) -> Self
    where
        W: Write + Send + 'static,
    {
        Self::new(Box::new(writer))
    }

    pub fn stdout() -> Self {
        Self::boxed(io::stdout())
    }
}

impl<W: Write> EventEmitter<W> {
    pub fn new(writer: W) -> Self {
        Self {
            writer: Mutex::new(BufWriter::new(writer)),
        }
    }

    pub fn emit(&self, event: &Event) -> io::Result<()> {
        let line = serde_json::to_vec(event).map_err(io::Error::other)?;
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| io::Error::other("event emitter writer lock poisoned"))?;

        writer.write_all(&line)?;
        writer.write_all(b"\n")?;
        writer.flush()
    }

    pub fn into_inner(self) -> io::Result<W> {
        let writer = self
            .writer
            .into_inner()
            .map_err(|_| io::Error::other("event emitter writer lock poisoned"))?;

        writer
            .into_inner()
            .map_err(|error| io::Error::new(error.error().kind(), error.to_string()))
    }
}

impl<W: Write> fmt::Debug for EventEmitter<W> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("EventEmitter")
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;
    use std::path::{Path, PathBuf};

    use chrono::TimeZone;
    use serde_json::json;

    fn timestamp() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 4, 23, 2, 30, 0)
            .single()
            .expect("valid timestamp")
    }

    fn event(kind: EventKind) -> Event {
        Event {
            kind,
            at: timestamp(),
            payload: json!({
                "threadId": "u-1",
                "text": "Looks good"
            }),
        }
    }

    #[test]
    fn event_kinds_serialize_to_dotted_strings() {
        let expected = [
            (EventKind::SessionStarted, "session.started"),
            (EventKind::ThreadCreated, "thread.created"),
            (EventKind::ThreadDeleted, "thread.deleted"),
            (EventKind::ThreadResolved, "thread.resolved"),
            (EventKind::ThreadUnresolved, "thread.unresolved"),
            (EventKind::ReplyAdded, "reply.added"),
            (EventKind::PromptSuggestDone, "prompt.suggest_done"),
            (EventKind::SessionDone, "session.done"),
        ];

        for (kind, wire_name) in expected {
            assert_eq!(
                serde_json::to_value(kind).expect("serialize event kind"),
                wire_name
            );
            assert_eq!(
                serde_json::from_value::<EventKind>(Value::String(wire_name.to_string()))
                    .expect("deserialize event kind"),
                kind
            );
        }
    }

    #[test]
    fn all_event_kinds_are_covered_by_the_wire_name_test() {
        let tested = [
            EventKind::SessionStarted,
            EventKind::ThreadCreated,
            EventKind::ThreadDeleted,
            EventKind::ThreadResolved,
            EventKind::ThreadUnresolved,
            EventKind::ReplyAdded,
            EventKind::PromptSuggestDone,
            EventKind::SessionDone,
        ];

        assert_eq!(tested, EventKind::ALL);
    }

    #[test]
    fn emit_writes_one_json_line_and_flushes() {
        let emitter = EventEmitter::new(Vec::new());
        let event = event(EventKind::ThreadCreated);

        emitter.emit(&event).expect("emit event");

        let output = String::from_utf8(emitter.into_inner().expect("writer")).expect("utf8");
        assert!(output.ends_with('\n'));
        assert_eq!(output.lines().count(), 1);

        let parsed: Event = serde_json::from_str(output.trim_end()).expect("parse emitted event");
        assert_eq!(parsed, event);
    }

    #[test]
    fn stdout_writes_are_isolated_to_events_module() {
        let src_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");
        let offenders = stdout_write_offenders(&src_dir);

        assert!(
            offenders.is_empty(),
            "stdout writes must stay in src/events.rs; offenders: {offenders:?}"
        );
    }

    fn stdout_write_offenders(src_dir: &Path) -> Vec<PathBuf> {
        let mut offenders = Vec::new();
        collect_stdout_write_offenders(src_dir, &mut offenders);
        offenders
    }

    fn collect_stdout_write_offenders(path: &Path, offenders: &mut Vec<PathBuf>) {
        let entries = fs::read_dir(path).expect("read src dir");

        for entry in entries {
            let entry = entry.expect("read dir entry");
            let path = entry.path();

            if path.is_dir() {
                collect_stdout_write_offenders(&path, offenders);
                continue;
            }

            if path.extension().and_then(|extension| extension.to_str()) != Some("rs")
                || path.file_name().and_then(|file_name| file_name.to_str()) == Some("events.rs")
            {
                continue;
            }

            let source = fs::read_to_string(&path).expect("read source file");

            if contains_stdout_write(&source) {
                offenders.push(path);
            }
        }
    }

    fn contains_stdout_write(source: &str) -> bool {
        source.contains("io::stdout")
            || source
                .match_indices("println!")
                .any(|(index, _)| !is_identifier_char_before(source, index))
            || source
                .match_indices("print!")
                .any(|(index, _)| !is_identifier_char_before(source, index))
    }

    fn is_identifier_char_before(source: &str, index: usize) -> bool {
        source[..index]
            .chars()
            .next_back()
            .is_some_and(|character| character == '_' || character.is_ascii_alphanumeric())
    }
}
