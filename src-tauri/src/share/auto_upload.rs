//! Automatic uploads: when to send a finished fight, and the retries after a failure.

use std::path::Path;

use super::slices::{meta_ids, load_sidecar, slice_bin_path, save_sidecar, SliceSidecar};
use super::upload::PublishError;
use crate::entity::fight_record::FightRecord;

/// The setting behind the Settings switch. Only the front end reads and
/// writes it; auto upload is on only when it holds `true`.
pub const AUTO_UPLOAD_KEY: &str = "dpsMeter.autoUpload";

/// How long to wait between tries while the keyring is locked.
const KEYRING_RETRY_MINUTES: i64 = 5;

/// Minutes until the next try after the first, second, ... sixth failure in
/// a row. There is no seventh try.
const BACKOFF_MINUTES: [i64; 6] = [1, 2, 5, 15, 30, 60];

const MINUTE_MS: i64 = 60_000;

/// Whether a fight the auto-save just wrote should go up now: a finished boss
/// fight with a kept slice that was never uploaded or tried. One that failed
/// before is the retry list's.
pub fn wants_auto_upload(app_data_dir: &Path, record: &FightRecord, finished: bool) -> bool {
    if record.is_train || !finished || !slice_bin_path(app_data_dir, &record.id).exists() {
        return false;
    }
    let meta = load_sidecar(app_data_dir, &record.id);
    meta.url.is_none() && meta.auto_attempts == 0
}

/// Books a failed automatic upload in the fight's meta: when to try again,
/// or that it is over. A locked keyring is no fault of the upload, so it
/// waits without counting.
pub fn note_auto_upload_failure(app_data_dir: &Path, id: &str, failure: &PublishError, now_ms: i64) {
    let mut meta = load_sidecar(app_data_dir, id);
    if failure.keyring_locked {
        meta.auto_attempts = meta.auto_attempts.max(1);
        meta.retry_at_ms = now_ms.saturating_add(KEYRING_RETRY_MINUTES * MINUTE_MS);
    } else {
        meta.auto_attempts = meta.auto_attempts.saturating_add(1);
        let wait = if failure.retryable { BACKOFF_MINUTES.get(meta.auto_attempts as usize - 1) } else { None };
        match wait {
            Some(minutes) => meta.retry_at_ms = now_ms.saturating_add(minutes * MINUTE_MS),
            None => meta.gave_up = true,
        }
    }
    save_sidecar(app_data_dir, id, &meta);
}

/// The fights whose failed automatic upload is due again by `now_ms`, in no
/// set order.
pub fn auto_upload_retries_due(app_data_dir: &Path, now_ms: i64) -> Vec<String> {
    meta_ids(app_data_dir)
        .into_iter()
        .filter(|id| due_again(&load_sidecar(app_data_dir, id), now_ms) && slice_bin_path(app_data_dir, id).exists())
        .collect()
}

/// A failed automatic upload, not yet given up, whose next try has come.
fn due_again(meta: &SliceSidecar, now_ms: i64) -> bool {
    meta.auto_attempts > 0 && meta.retry_at_ms <= now_ms && !meta.gave_up && meta.url.is_none()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::built::Scratch;
    use crate::share::slices::slice_folder;
    use crate::share::upload::tests::saved_fight;

    const T: i64 = 1_000_000;

    /// A fresh folder with an empty `slices/`, gone when the test ends.
    struct Data(Scratch);

    impl Data {
        fn new(tag: &str) -> Self {
            let data = Data(Scratch::new(&format!("auto-upload.{tag}")));
            std::fs::create_dir_all(slice_folder(&data.0)).expect("scratch folder");
            data
        }

        fn slice(&self, id: &str) -> &Self {
            std::fs::write(slice_bin_path(&self.0, id), "made-up slice").expect("slice file");
            self
        }

        fn meta(&self, id: &str, meta: SliceSidecar) -> &Self {
            save_sidecar(&self.0, id, &meta);
            self
        }

        fn due(&self, now: i64) -> Vec<String> {
            let mut ids = auto_upload_retries_due(&self.0, now);
            ids.sort();
            ids
        }

        fn fail(&self, id: &str, how: &PublishError, now: i64) -> SliceSidecar {
            note_auto_upload_failure(&self.0, id, how, now);
            load_sidecar(&self.0, id)
        }
    }

    fn failure(retryable: bool, keyring_locked: bool) -> PublishError {
        PublishError { message: "made-up failure".into(), retryable, keyring_locked }
    }

    fn train(mut record: FightRecord) -> FightRecord {
        record.is_train = true;
        record
    }

    #[test]
    fn only_an_untried_fight_goes_up() {
        let data = Data::new("choice");
        data.slice("f1");
        let fight = saved_fight("f1", &[]);
        data.meta("f1", SliceSidecar::default());
        assert!(wants_auto_upload(&data.0, &fight, true));
        data.meta("f1", SliceSidecar { auto_attempts: 1, ..SliceSidecar::default() });
        assert!(!wants_auto_upload(&data.0, &fight, true));
        data.meta("f1", SliceSidecar { gave_up: true, ..SliceSidecar::default() });
        assert!(wants_auto_upload(&data.0, &fight, true), "giving up is not looked at");
    }

    #[test]
    fn an_auto_saved_fight_waits_for_its_end_and_goes_once() {
        let data = Data::new("once");
        let fight = saved_fight("auto_9_1000", &[]);
        assert!(!wants_auto_upload(&data.0, &fight, true));
        data.slice("auto_9_1000");
        assert!(!wants_auto_upload(&data.0, &fight, false));
        assert!(wants_auto_upload(&data.0, &fight, true));
        assert!(!wants_auto_upload(&data.0, &train(fight.clone()), true));
        data.meta("auto_9_1000", SliceSidecar { url: Some("https://a2tools.app/logs/x".into()), ..SliceSidecar::default() });
        assert!(!wants_auto_upload(&data.0, &fight, true));
    }

    #[test]
    fn each_failure_waits_its_own_delay_and_the_last_keeps_it() {
        let data = Data::new("backoff");
        data.slice("f1").meta("f1", SliceSidecar::default());
        let waits: Vec<_> = (0..6)
            .map(|_| data.fail("f1", &failure(true, false), T))
            .map(|meta| (meta.auto_attempts, meta.retry_at_ms))
            .collect();
        assert_eq!(
            waits,
            [(1, 1_060_000), (2, 1_120_000), (3, 1_300_000), (4, 1_900_000), (5, 2_800_000), (6, 4_600_000)]
        );
        let last = data.fail("f1", &failure(true, false), T);
        assert_eq!((last.auto_attempts, last.retry_at_ms), (7, 4_600_000));
    }

    #[test]
    fn a_locked_keyring_waits_without_counting() {
        let data = Data::new("locked");
        data.slice("f1").meta("f1", SliceSidecar::default());
        let mut meta = SliceSidecar::default();
        for _ in 0..50 {
            meta = data.fail("f1", &failure(true, true), T);
        }
        assert_eq!((meta.auto_attempts, meta.gave_up, meta.retry_at_ms), (1, false, 1_300_000));
        assert_eq!(data.due(T + 300_000), ["f1"]);
        let after = data.fail("f1", &failure(true, false), 2_000_000);
        assert_eq!((after.auto_attempts, after.retry_at_ms), (2, 2_120_000));

        data.meta("f1", SliceSidecar { auto_attempts: 3, ..SliceSidecar::default() });
        assert_eq!(data.fail("f1", &failure(true, true), T).auto_attempts, 3);
    }

    #[test]
    fn a_failure_without_a_slices_folder_writes_nothing() {
        let data = Data::new("no-folder");
        std::fs::remove_dir_all(slice_folder(&data.0)).expect("folder removed");
        note_auto_upload_failure(&data.0, "f1", &failure(true, false), T);
        note_auto_upload_failure(&data.0, "f1", &failure(true, true), T);
        assert!(!slice_folder(&data.0).exists());
    }

    #[test]
    fn the_retry_list_holds_only_fights_due_again() {
        let data = Data::new("due");
        assert!(data.due(i64::MAX).is_empty());
        let waiting = SliceSidecar { auto_attempts: 1, retry_at_ms: T, ..SliceSidecar::default() };
        data.slice("due").meta("due", waiting.clone());
        data.slice("over").meta("over", SliceSidecar { gave_up: true, ..waiting.clone() });
        data.slice("sent").meta("sent", SliceSidecar { url: Some("u".into()), ..waiting.clone() });
        data.slice("untried").meta("untried", SliceSidecar { auto_attempts: 0, ..waiting.clone() });
        data.meta("no-slice", waiting.clone());
        data.slice("no-meta");
        std::fs::write(slice_folder(&data.0).join("stray.txt"), "x").expect("stray file");

        assert!(data.due(T - 1).is_empty());
        assert_eq!(data.due(T), ["due"]);
        assert_eq!(data.due(i64::MAX), ["due"]);
    }

    #[test]
    fn no_slices_folder_means_nothing_is_due() {
        let data = Data::new("absent");
        std::fs::remove_dir_all(slice_folder(&data.0)).expect("folder removed");
        assert!(auto_upload_retries_due(&data.0, i64::MAX).is_empty());
    }

    #[test]
    fn a_fight_is_retried_on_schedule_until_it_gives_up() {
        let data = Data::new("schedule");
        data.slice("f1").meta("f1", SliceSidecar::default());
        assert!(data.due(T).is_empty(), "never failed");
        data.fail("f1", &failure(true, false), T);
        assert!(data.due(T + 59_000).is_empty());
        assert_eq!(data.due(T + 60_000), ["f1"]);
        for _ in 2..=6 {
            assert!(!data.fail("f1", &failure(true, false), T).gave_up);
        }
        assert!(data.fail("f1", &failure(true, false), T).gave_up);
        assert!(data.due(i64::MAX).is_empty());

        data.slice("f2").meta("f2", SliceSidecar::default());
        let fatal = data.fail("f2", &failure(false, false), T);
        assert_eq!((fatal.auto_attempts, fatal.gave_up, fatal.retry_at_ms), (1, true, 0));
    }

    #[test]
    fn deciding_and_writing_need_no_network() {
        let data = Data::new("offline");
        data.slice("f1").meta("f1", SliceSidecar::default());
        assert!(wants_auto_upload(&data.0, &saved_fight("f1", &[]), true));
        note_auto_upload_failure(&data.0, "f1", &failure(true, false), T);
        assert_eq!(data.due(T + 60_000), ["f1"]);
    }
}
