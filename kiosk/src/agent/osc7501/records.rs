//! The records one terminal holds, as the Program Status Protocol defines them.

use super::report::{Record, RecordId, Report};

/// The protocol's cap on records per terminal.
const MAX_RECORDS: usize = 256;

struct Entry {
    id: RecordId,
    record: Record,
    updated: u64,
}

#[derive(Default)]
pub struct RecordSet {
    entries: Vec<Entry>,
    clock: u64,
}

impl RecordSet {
    pub fn apply(&mut self, report: Report) {
        match report {
            Report::Set { id, record } => {
                self.clock += 1;
                let updated = self.clock;
                if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
                    entry.record = record;
                    entry.updated = updated;
                    return;
                }
                if self.entries.len() == MAX_RECORDS
                    && let Some(oldest) = self
                        .entries
                        .iter()
                        .enumerate()
                        .min_by_key(|(_, entry)| entry.updated)
                        .map(|(index, _)| index)
                {
                    self.entries.swap_remove(oldest);
                }
                self.entries.push(Entry {
                    id,
                    record,
                    updated,
                });
            }
            Report::Clear { id } => self.entries.retain(|entry| !entry.id.is_within(&id)),
            Report::Query => {}
        }
    }

    /// A new shell prompt began: the program that reported working or blocked is done.
    pub fn prompt_started(&mut self) {
        self.entries
            .retain(|entry| !entry.record.state.ends_with_program());
    }

    /// A full terminal reset removes every record.
    pub fn reset(&mut self) {
        self.entries.clear();
    }

    /// The record that most wants the user's attention, the most recent among equals, with
    /// `app` inherited from its nearest ancestor when it has none.
    pub fn most_urgent(&self) -> Option<Record> {
        let entry = self
            .entries
            .iter()
            .max_by_key(|entry| (entry.record.state.urgency(), entry.updated))?;
        let mut record = entry.record.clone();
        if record.app.is_none() {
            record.app = entry.id.ancestors().find_map(|ancestor| {
                self.entries
                    .iter()
                    .find(|other| other.id == ancestor)
                    .and_then(|other| other.record.app.clone())
            });
        }
        Some(record)
    }
}

#[cfg(test)]
mod tests {
    use super::{super::report::parse, *};

    fn apply(records: &mut RecordSet, body: &str) {
        records.apply(parse(body).unwrap());
    }

    fn summary(records: &RecordSet) -> Option<String> {
        records.most_urgent().map(|record| record.to_string())
    }

    #[test]
    fn each_report_replaces_its_record() {
        let mut records = RecordSet::default();
        apply(&mut records, "state=working:app=brew");
        apply(&mut records, "state=done");
        assert_eq!(summary(&records).as_deref(), Some("state=done"));
    }

    #[test]
    fn the_most_urgent_record_wins_and_children_inherit_app() {
        let mut records = RecordSet::default();
        apply(&mut records, "state=working:app=deploy");
        apply(&mut records, "state=blocked:kind=permission:id=eu-west");
        assert_eq!(
            summary(&records).as_deref(),
            Some("state=blocked:kind=permission:app=deploy")
        );
        apply(&mut records, "state=clear:id=eu-west");
        assert_eq!(
            summary(&records).as_deref(),
            Some("state=working:app=deploy")
        );
        apply(&mut records, "state=clear");
        assert_eq!(summary(&records), None);
    }

    #[test]
    fn clearing_a_parent_clears_its_children() {
        let mut records = RecordSet::default();
        apply(&mut records, "state=working:id=build/test");
        apply(&mut records, "state=idle:id=builder");
        apply(&mut records, "state=clear:id=build");
        assert_eq!(summary(&records).as_deref(), Some("state=idle"));
    }

    #[test]
    fn a_prompt_drops_only_working_and_blocked_records() {
        let mut records = RecordSet::default();
        apply(&mut records, "state=working");
        apply(&mut records, "state=done:id=a");
        apply(&mut records, "state=blocked:id=b");
        records.prompt_started();
        assert_eq!(summary(&records).as_deref(), Some("state=done"));
        records.reset();
        assert_eq!(summary(&records), None);
    }

    #[test]
    fn the_least_recently_updated_record_makes_room() {
        let mut records = RecordSet::default();
        apply(&mut records, "state=blocked:id=first");
        for index in 0..MAX_RECORDS {
            apply(&mut records, &format!("state=idle:id=r{index}"));
        }
        assert_eq!(records.entries.len(), MAX_RECORDS);
        assert_eq!(summary(&records).as_deref(), Some("state=idle"));
    }
}
